use std::time::{Duration, Instant};

use game::Delivery;
use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Record, ServerMessage, VERSION,
    Welcome, Why,
};
use server::{Config, InProcess, InputOrder, Spawn, Standing, Stepper};

fn send(client: &InProcess, message: &ClientMessage) {
    let mut bytes = Vec::new();
    message.write(&mut bytes);
    client.send(bytes).expect("the server reads it");
}

fn hello(name: &str) -> ClientMessage {
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: name.into(),
        appearance: Appearance::default(),
    })
}

#[derive(Default)]
struct Heard {
    handed_at: Vec<Instant>,
    welcome: Option<Welcome>,
    granted: bool,
    refused: Option<Why>,
}

fn heard(client: &InProcess) -> Heard {
    let mut heard = Heard::default();
    let mut frames = Frames::default();
    while let Ok((bytes, at)) = client.try_recv() {
        heard.handed_at.push(at);
        frames.extend(&bytes);
        while let Some(frame) = frames.next_frame().expect("whole frames") {
            let batch = match ServerMessage::read(frame).expect("a message") {
                ServerMessage::Welcome(w) => {
                    heard.welcome = Some(w);
                    continue;
                }
                ServerMessage::Batch(batch) => batch,
            };
            for record in batch.map(|r| r.expect("a valid record")) {
                match record {
                    Record::Granted { .. } => heard.granted = true,
                    Record::Correct { why, .. } => heard.refused = Some(why),
                    _ => {}
                }
            }
        }
    }
    heard
}

#[test]
fn a_connection_from_inside_is_read_and_answered_when_its_caller_says_as_a_hosts_or_a_guests() {
    let cfg = Config {
        tick_threads: 1,
        spawns: [0.0, 3.0]
            .map(|x| Spawn {
                pos: [x, 0.0, 0.0],
                facing: 0.0,
            })
            .to_vec(),
        ..Config::default()
    };
    let mut stepper =
        Stepper::new(&cfg, InputOrder::Canonical, Delivery::Canonical).expect("a stepper");
    let (host, guest) = (
        stepper.connect_in_process(Standing::Host),
        stepper.connect_in_process(Standing::Guest),
    );
    send(&host, &hello("Host"));
    send(&guest, &hello("Guest"));
    let epoch = Instant::now();
    stepper.tick(&[]);
    stepper.hand_over(epoch);
    assert!(
        heard(&host).handed_at.is_empty(),
        "a hello not yet received"
    );

    stepper.receive(50);
    stepper.tick(&[]);
    assert!(
        heard(&host).handed_at.is_empty(),
        "a welcome not yet handed over"
    );
    let handed = epoch + Duration::from_millis(100);
    stepper.hand_over(handed);
    let (to_host, to_guest) = (heard(&host), heard(&guest));
    let handed_at = to_host.handed_at.iter().chain(&to_guest.handed_at);
    assert!(handed_at.copied().all(|at| at == handed));
    let host_id = to_host.welcome.expect("the host's welcome").id;
    let guest_id = to_guest.welcome.expect("the guest's welcome").id;

    let far = Claim {
        ack: 0,
        movement: Movement {
            time: 1000,
            pos: [500.0, 0.0, 0.0],
            ..Movement::default()
        },
    };
    send(&host, &ClientMessage::Teleport(far));
    send(&guest, &ClientMessage::Teleport(far));
    stepper.receive(1000);
    stepper.tick(&[]);
    stepper.hand_over(handed);
    let (to_host, to_guest) = (heard(&host), heard(&guest));
    assert!(to_host.granted && to_host.refused.is_none());
    assert!(!to_guest.granted && to_guest.refused == Some(Why::Teleport));
    let stands = |s: &Stepper, id| s.body(id).map(|b| b.pos);
    assert_eq!(stands(&stepper, host_id), Some([500.0, 0.0, 0.0]));
    assert_eq!(stands(&stepper, guest_id), Some([3.0, 0.0, 0.0]));

    drop(guest);
    stepper.tick(&[]);
    assert!(
        stepper.body(guest_id).is_some(),
        "a hang-up not yet received"
    );
    stepper.receive(1100);
    stepper.tick(&[]);
    assert!(stepper.body(guest_id).is_none(), "the guest hung up");
}
