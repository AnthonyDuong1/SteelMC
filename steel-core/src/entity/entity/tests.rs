use std::sync::Arc;

use glam::DVec3;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use crate::behavior::init_behaviors;
use crate::entity::entities::PigEntity;
use crate::entity::{EntityBase, RemovalReason, SharedEntity};
use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

use super::look_at_rotation;

#[test]
fn look_at_rotation_matches_vanilla_axes() {
    assert_eq!(
        look_at_rotation(DVec3::ZERO, DVec3::new(0.0, 0.0, 1.0)),
        (0.0, 0.0)
    );
    assert_eq!(
        look_at_rotation(DVec3::ZERO, DVec3::new(1.0, 0.0, 0.0)),
        (-90.0, 0.0)
    );
    assert_eq!(
        look_at_rotation(DVec3::ZERO, DVec3::new(0.0, 1.0, 1.0)),
        (0.0, -45.0)
    );
    assert_eq!(
        look_at_rotation(DVec3::ZERO, DVec3::new(-1.0, 0.0, -1.0)),
        (135.0, 0.0)
    );
}

fn riding_pigs(name: &'static str) -> (Arc<World>, SharedEntity, SharedEntity) {
    init_vanilla_registry();
    init_behaviors();

    let world = fresh_test_world(name);
    insert_ready_full_chunk(&world, ChunkPos::new(0, 0));

    // Give all eight horizontal dismount candidates a solid floor.
    for x in 7..=9 {
        for z in 7..=9 {
            assert!(world.set_block(
                BlockPos::new(x, 64, z),
                vanilla_blocks::STONE.default_state(),
                UpdateFlags::UPDATE_NONE,
            ));
        }
    }

    let position = DVec3::new(8.5, 65.0, 8.5);
    let vehicle: SharedEntity = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        1,
        position,
        Arc::downgrade(&world),
    ));
    let passenger: SharedEntity = Arc::new(PigEntity::new(
        &vanilla_entities::PIG,
        2,
        position,
        Arc::downgrade(&world),
    ));

    world
        .try_add_entity(Arc::clone(&vehicle))
        .expect("vehicle should attach to the loaded chunk");
    world
        .try_add_entity(Arc::clone(&passenger))
        .expect("passenger should attach to the loaded chunk");

    // Yaw zero faces south, so the first dismount candidate is west.
    vehicle.set_rotation((0.0, 0.0));

    EntityBase::restore_passenger_relationship(&vehicle, &passenger);
    vehicle
        .position_rider(passenger.as_ref())
        .expect("passenger should be positioned on its vehicle");

    assert!(passenger.is_passenger());
    assert!(vehicle.has_passenger(passenger.as_ref()));

    (world, vehicle, passenger)
}

#[test]
fn dismount_clears_relationships_and_uses_first_safe_offset() {
    let (_world, vehicle, passenger) = riding_pigs("dismount_first_safe_offset");

    passenger.stop_riding();

    assert!(!passenger.is_passenger());
    assert!(vehicle.passengers().is_empty());
    assert_eq!(passenger.base().boarding_cooldown(), 60);
    assert_eq!(passenger.position(), DVec3::new(7.5, 65.0, 8.5));

    let position_after_dismount = passenger.position();
    passenger.stop_riding();
    assert_eq!(passenger.position(), position_after_dismount);
}

#[test]
fn dismount_skips_blocked_first_offset() {
    let (world, vehicle, passenger) = riding_pigs("dismount_blocked_first_offset");

    // Block west, the first candidate for a south-facing vehicle.
    assert!(world.set_block(
        BlockPos::new(7, 65, 8),
        vanilla_blocks::STONE.default_state(),
        UpdateFlags::UPDATE_NONE,
    ));

    passenger.stop_riding();

    // East is the second candidate.
    assert_eq!(passenger.position(), DVec3::new(9.5, 65.0, 8.5));
    assert!(!passenger.is_passenger());
    assert!(vehicle.passengers().is_empty());
}

#[test]
fn dismount_of_removed_passenger_preserves_position() {
    let (_world, vehicle, passenger) = riding_pigs("dismount_removed_passenger");
    let riding_position = passenger.position();

    passenger.set_removed(RemovalReason::Discarded);

    assert_eq!(passenger.removal_reason(), Some(RemovalReason::Discarded),);
    assert!(!passenger.is_passenger());
    assert!(vehicle.passengers().is_empty());

    // Vanilla preserves the current position when dismounting a removed passenger.
    assert_eq!(passenger.position(), riding_position);
}
