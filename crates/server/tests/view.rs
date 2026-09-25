use std::io::ErrorKind;

use game::Delivery;
use server::{Config, InputOrder, Limits, PastReach, Stepper, View};

fn config(radius: f32, run: f32) -> Config {
    Config {
        view: View {
            radius,
            ..View::default()
        },
        limits: Limits {
            run,
            ..Limits::default()
        },
        tick_threads: 1,
        io_threads: 1,
        ..Config::default()
    }
}

#[test]
fn a_view_past_what_the_wire_reaches_is_refused_by_name_and_the_same_within_it_runs() {
    let run = Limits::default().run;
    for (radius, run, reach) in [(250.0, run, 242.0), (230.0, 14.0, 228.0)] {
        let past = PastReach {
            view_yd: radius + View::default().grey,
            reach_yd: reach,
        };
        let cfg = config(radius, run);
        assert_eq!(cfg.view.check(&cfg.limits), Err(past));
        let refused = server::start(cfg.clone())
            .err()
            .expect("a view past reach refused");
        assert_eq!(refused.kind(), ErrorKind::InvalidInput);
        assert_eq!(refused.to_string(), past.to_string());
        let refused = Stepper::new(&cfg, InputOrder::Canonical, Delivery::Canonical)
            .err()
            .expect("a view past reach refused");
        assert_eq!(refused.to_string(), past.to_string());

        let within = config(reach - 1.0 - View::default().grey, run);
        assert_eq!(within.view.check(&within.limits), Ok(()));
        server::start(within.clone())
            .expect("a view within reach")
            .stop()
            .expect("it stops");
        Stepper::new(&within, InputOrder::Canonical, Delivery::Canonical)
            .expect("a view within reach");
    }
    assert!(
        PastReach {
            view_yd: 251.0,
            reach_yd: 242.0
        }
        .to_string()
        .contains("251.0 yd, its radius and grey, goes past the 242.0 yd"),
    );
}
