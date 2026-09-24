use std::time::{Duration, Instant};

use kira::backend::mock::{MockBackend, MockBackendSettings};

use super::stream_watch::Verdict;
use super::*;

const RATE: u32 = 44_100;

fn preset(decay: f32, hf_ratio: f32, room: i32, room_hf: i32, reverb: i32) -> SoundProvider {
    SoundProvider {
        id: 0,
        name: String::new(),
        flags: 0,
        decay_time: decay,
        room,
        room_hf,
        decay_hf_ratio: hf_ratio,
        reflections: 0,
        reverb,
        env_diffusion: 1.0,
        env_size: 1.0,
    }
}

#[test]
fn the_reverb_projection_orders_the_presets() {
    let (fg, dg, wg) = freeverb_projection(&preset(1.49, 0.86, -1000, -100, 200));
    let (fc, _, _) = freeverb_projection(&preset(3.0, 1.3, -1000, -500, -402));
    let (fh, _, _) = freeverb_projection(&preset(10.05, 0.26, -1000, -1000, 198));
    assert!(fg < fc && fc < fh && fh <= 0.98);
    assert!((wg.0 + 8.0).abs() < 1e-3);
    assert!((0.0..=1.0).contains(&dg));
    let (_, du, wu) = freeverb_projection(&preset(1.0, 0.1, -1000, -10000, 1700));
    assert!(du > 0.9);
    assert!((wu.0 - 6.0).abs() < 1e-3);
    let (_, _, off) = freeverb_projection(&preset(1.0, 1.0, -10000, -10000, 200));
    assert_eq!(off, Decibels::SILENCE);
}

/// A quarter second of a full-scale stereo float sine.
fn full_scale_tone(rate: u32) -> Vec<u8> {
    let frames = rate as usize / 4;
    let mut wav = crate::mix_tap::wav_header(rate, frames as u64 * 2).to_vec();
    for i in 0..frames {
        let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
        wav.extend_from_slice(&v.to_le_bytes());
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav
}

fn manager_over(main: kira::track::MainTrackBuilder) -> AudioManager<MockBackend> {
    AudioManager::<MockBackend>::new(AudioManagerSettings {
        backend_settings: MockBackendSettings { sample_rate: RATE },
        main_track_builder: main,
        ..Default::default()
    })
    .expect("mock backend")
}

fn run(manager: &mut AudioManager<MockBackend>, secs: f64) {
    let backend = manager.backend_mut();
    for _ in 0..((secs * f64::from(RATE) / 128.0).ceil() as usize) {
        backend.on_start_processing();
        backend.process();
    }
}

#[test]
fn the_output_gate_silences_the_output_and_nothing_upstream() {
    let asked = Arc::new(MixLevel::default());
    let heard = Arc::new(MixLevel::default());
    let on = Arc::new(AtomicBool::new(true));
    let chain = main_track(&asked, &on, Some(RATE), None);
    let (mut main, mut output) = (chain.builder, chain.output);
    meter::install(&mut main, &heard);
    let mut manager = manager_over(main);
    let data = sfx_from_bytes(full_scale_tone(RATE)).expect("tone decodes");
    manager.play(data.clone()).expect("play");
    run(&mut manager, 0.3);
    assert!(asked.take().peak > 0.9 && heard.take().peak > 0.9);
    output.set_volume(Decibels::SILENCE, glide());
    run(&mut manager, 0.05);
    let _ = (asked.take(), heard.take());
    manager.play(data).expect("play");
    run(&mut manager, 0.3);
    assert!(heard.take().peak < 1e-3, "the gate is shut");
    assert!(asked.take().peak > 0.9, "the meter still reads the mix");
}

#[test]
fn the_master_sits_ahead_of_the_limiter() {
    const MASTER: f32 = 0.25;
    let asked = Arc::new(MixLevel::default());
    let heard = Arc::new(MixLevel::default());
    let on = Arc::new(AtomicBool::new(true));
    let chain = main_track(&asked, &on, Some(RATE), None);
    let (mut main, mut master) = (chain.builder, chain.master);
    meter::install(&mut main, &heard);
    let mut manager = manager_over(main);
    master.set_volume(amp_to_db(MASTER), snap());
    run(&mut manager, 0.02);
    let _ = (asked.take(), heard.take());
    let data = sfx_from_bytes(full_scale_tone(RATE)).expect("tone decodes");
    manager.play(data.clone()).expect("play");
    manager.play(data).expect("play");
    run(&mut manager, 0.5);
    let (inner, heard) = (asked.take(), heard.take());
    assert!(inner.reduction > 0.99, "limited {}", inner.reduction);
    assert!((heard.peak - 2.0 * MASTER).abs() < 0.05, "{}", heard.peak);
}

#[test]
fn the_spatial_arena_refuses_past_its_capacity() {
    fn fill(capacity: usize) -> usize {
        let mut manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
            backend_settings: MockBackendSettings { sample_rate: RATE },
            capacities: kira::Capacities {
                sub_track_capacity: capacity,
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("mock backend");
        let listener = manager
            .add_listener(mint_vec(Vec3::ZERO), mint_quat(Quat::IDENTITY))
            .expect("listener");
        let mut held = Vec::new();
        loop {
            match manager.add_spatial_sub_track(
                listener.id(),
                mint_vec(Vec3::ZERO),
                SpatialTrackBuilder::new().sound_capacity(1),
            ) {
                Ok(t) => held.push(t),
                Err(_) => return held.len(),
            }
        }
    }
    assert_eq!(fill(kira::Capacities::default().sub_track_capacity), 128);
    assert_eq!(fill(SPATIAL_VOICE_CAPACITY), SPATIAL_VOICE_CAPACITY);
}

#[test]
fn five_aligned_full_scale_copies_do_not_clip() {
    let asked = Arc::new(MixLevel::default());
    let heard = Arc::new(MixLevel::default());
    let on = Arc::new(AtomicBool::new(true));
    let mut main = main_track(&asked, &on, Some(RATE), None).builder;
    meter::install(&mut main, &heard);
    let mut manager = manager_over(main);
    let data = sfx_from_bytes(full_scale_tone(RATE)).expect("tone decodes");
    for _ in 0..5 {
        manager.play(data.clone()).expect("play");
    }
    run(&mut manager, 0.5);
    let heard = heard.take();
    assert!(asked.take().peak > 4.0);
    assert_eq!(heard.over, 0);
    assert!(heard.peak <= crate::limiter::ceiling() + 1e-5);
}

fn wall(t0: Instant, secs: f64) -> Instant {
    t0 + Duration::from_secs_f64(secs)
}

#[test]
fn the_stream_watch_counts_freezes_not_swaps_or_spin_up() {
    let dt = 1.0 / 60.0;
    let t0 = Instant::now();
    let mut w = StreamWatch::new("test");
    let mut pos = 0.0;
    let mut starved = Vec::new();
    for i in 0..90 {
        if !(30..48).contains(&i) {
            pos += dt;
        }
        if let Some(Verdict::Starved { lost, .. }) =
            w.observe(true, pos, dt, wall(t0, f64::from(i) * dt))
        {
            starved.push(lost);
        }
    }
    assert_eq!(starved.len(), 1);
    assert!((starved[0] - 0.3).abs() < 0.05);

    let mut w = StreamWatch::new("test");
    let mut pos = 40.0;
    for i in 0..120 {
        if i == 30 {
            pos = 0.0;
        }
        let v = w.observe(true, pos, dt, wall(t0, f64::from(i) * dt));
        assert!(
            !matches!(v, Some(Verdict::Starved { .. })),
            "a swap is not a freeze"
        );
        pos += dt;
    }

    let mut w = StreamWatch::new("test");
    let (hitch, first) = (0.298, 0.233);
    let mut ticks = vec![0.0, hitch];
    ticks.extend((1..=180).map(|i| hitch + f64::from(i) * dt));
    let mut began = 0;
    for (i, t) in ticks.iter().enumerate() {
        let delta = if i == 0 { dt } else { t - ticks[i - 1] };
        match w.observe(true, (t - first).max(0.0), delta, wall(t0, *t)) {
            Some(Verdict::Starved { .. }) => panic!("the spin-up was charged"),
            Some(Verdict::Began { .. }) => began += 1,
            _ => {}
        }
    }
    assert_eq!(began, 1);

    let mut w = StreamWatch::new("test");
    let never = (0..600)
        .filter(|&i| {
            matches!(
                w.observe(true, 0.0, dt, wall(t0, f64::from(i) * dt)),
                Some(Verdict::NeverStarted { .. })
            )
        })
        .count();
    assert_eq!(never, 1);
}

#[test]
fn a_fade_stop_survives_dropping_the_handle() {
    struct Capture(Arc<std::sync::Mutex<Option<kira::backend::Renderer>>>);
    impl kira::backend::Backend for Capture {
        type Settings = ();
        type Error = ();
        fn setup((): (), _: usize) -> Result<(Self, u32), ()> {
            Ok((Capture(Arc::default()), 100))
        }
        fn start(&mut self, renderer: kira::backend::Renderer) -> Result<(), ()> {
            *self.0.lock().expect("unpoisoned") = Some(renderer);
            Ok(())
        }
    }
    let mut manager =
        AudioManager::<Capture>::new(AudioManagerSettings::default()).expect("manager");
    let slot = manager.backend_mut().0.clone();
    let render = |n: usize| -> f32 {
        let mut guard = slot.lock().expect("unpoisoned");
        let r = guard.as_mut().expect("started");
        r.on_start_processing();
        let mut out = vec![0.0f32; n * 2];
        r.process(&mut out, 2);
        out.iter().step_by(2).map(|s| s.abs()).sum::<f32>() / n as f32
    };
    let frames: Arc<[kira::Frame]> = (0..100).map(|_| kira::Frame::from_mono(1.0)).collect();
    let bed = StaticSoundData {
        sample_rate: 100,
        frames,
        settings: kira::sound::static_sound::StaticSoundSettings::default(),
        slice: None,
    }
    .loop_region(..);
    let mut h = manager.play(bed).expect("play");
    assert!(render(5) > 0.9);
    h.stop(fade(1000));
    drop(h);
    let series: Vec<f32> = (0..26).map(|_| render(5)).collect();
    assert!(*series.last().expect("blocks") < 0.05);
    assert!(series.iter().filter(|&&a| (0.05..0.9).contains(&a)).count() >= 3);
}
