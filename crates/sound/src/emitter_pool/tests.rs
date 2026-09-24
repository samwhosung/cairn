use super::*;

fn owners(n: u32) -> Vec<Entity> {
    (0..n)
        .map(|i| Entity::from_raw_u32(i).expect("id"))
        .collect()
}

fn walk(pool: &AmbientEmitterPool, live: impl Fn(usize) -> bool) -> (Vec<usize>, Vec<usize>) {
    let (mut sounding, mut withheld) = (Vec::new(), Vec::new());
    for e in 0..POOL_ENTRIES {
        match cap_step(pool.entries[e].entitled(), sounding.len()) {
            CapStep::Skip => {}
            CapStep::Retire => withheld.push(e),
            CapStep::Service => {
                if live(e) {
                    sounding.push(e);
                }
            }
        }
    }
    (sounding, withheld)
}

#[test]
fn every_doodad_naming_one_id_shares_one_entry_heard_from_the_nearest() {
    let mut pool = AmbientEmitterPool::default();
    for (i, d) in owners(30).into_iter().enumerate() {
        pool.register(d, 3378, Vec3::new(i as f32, 0.0, 0.0), Vec3::ZERO);
    }
    assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
    assert_eq!(pool.entries[0].records.len(), 30);
    assert_eq!(
        pool.entries[0]
            .nearest(Vec3::new(29.0, 0.0, 0.0))
            .map(|p| p.x),
        Some(29.0)
    );
    let ties = owners(2);
    let mut pool = AmbientEmitterPool::default();
    pool.register(ties[0], 7, Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
    pool.register(ties[1], 7, Vec3::new(0.0, 0.0, -5.0), Vec3::ZERO);
    assert_eq!(pool.entries[0].nearest(Vec3::ZERO).map(|p| p.z), Some(5.0));
}

#[test]
fn the_same_id_moves_the_record_and_another_takes_its_place() {
    let mut pool = AmbientEmitterPool::default();
    let d = owners(1)[0];
    pool.register(d, 3378, Vec3::ZERO, Vec3::ZERO);
    pool.register(d, 3378, Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO);
    assert_eq!(pool.entries[0].records.len(), 1);
    assert_eq!(pool.entries[0].records[0].pos, Vec3::new(1.0, 2.0, 3.0));
    pool.register(d, 2000, Vec3::ZERO, Vec3::ZERO);
    assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
    pool.release(d);
    assert!(pool.entries.iter().all(|e| e.id == 0) && pool.handles.is_empty());
}

#[test]
fn a_thirty_third_id_waits_and_a_full_entry_evicts_the_first_farther() {
    let mut pool = AmbientEmitterPool::default();
    let ds = owners(POOL_ENTRIES as u32 + 1);
    for (i, d) in ds.iter().enumerate() {
        pool.register(*d, 100 + i as u32, Vec3::ZERO, Vec3::ZERO);
    }
    assert!(!pool.handles.contains_key(&ds[POOL_ENTRIES]));
    let mut pool = AmbientEmitterPool::default();
    let ds = owners(RECORDS_PER_ENTRY as u32 + 1);
    for (i, d) in ds.iter().take(RECORDS_PER_ENTRY).enumerate() {
        let x = (RECORDS_PER_ENTRY - 1 - i) as f32;
        pool.register(*d, 9, Vec3::new(x, 0.0, 0.0), Vec3::ZERO);
    }
    pool.register(
        ds[RECORDS_PER_ENTRY],
        9,
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::ZERO,
    );
    assert!(!pool.handles.contains_key(&ds[0]));
    assert_eq!(pool.entries[0].records.len(), RECORDS_PER_ENTRY);
}

#[test]
fn four_sound_by_claim_order_and_a_silent_entry_holds_no_slot() {
    let mut pool = AmbientEmitterPool::default();
    let ds = owners(6);
    for (i, d) in ds.iter().enumerate() {
        pool.register(*d, 10 * (i as u32 + 1), Vec3::ZERO, Vec3::ZERO);
    }
    assert_eq!(walk(&pool, |_| true), (vec![0, 1, 2, 3], vec![4, 5]));
    assert_eq!(walk(&pool, |e| e == 4), (vec![4], vec![]));
}
