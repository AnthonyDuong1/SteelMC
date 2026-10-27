//! Boat entity.
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::AbstractBoatEntityData;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, WorldAabb};

use super::super::VehicleEntity;
use super::abstract_boat::{AbstractBoat, AbstractBoatBase};
use crate::behavior::InteractionResult;
use crate::entity::leash::{LeashData, Leashable};
use crate::entity::{
    DamageSource, Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityMovementEmission,
    EntitySyncedData, RemovalReason, SharedEntity,
};
use crate::player::Player;
use crate::world::World;

/// A regular wooden boat, with variant properties supplied by extracted data.
#[entity_behavior(class = "Boat")]
pub struct BoatEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<AbstractBoatEntityData>,
    boat_base: AbstractBoatBase,
    #[json_arg(vanilla_items)]
    drop_item: ItemRef,
}

// SAFETY: Steel owns this key, which uniquely identifies the concrete `BoatEntity`.
unsafe impl DowncastType for BoatEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/boat");
}

impl BoatEntity {
    /// Creates a new boat entity.
    #[must_use]
    pub fn new(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        world: Weak<World>,
        drop_item: ItemRef,
    ) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat_base: AbstractBoatBase::new(),
            drop_item,
        }
    }

    /// Creates a boat entity from saved data.
    #[must_use]
    pub fn from_saved(
        entity_type: EntityTypeRef,
        load: EntityBaseLoad,
        drop_item: ItemRef,
    ) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat_base: AbstractBoatBase::new(),
            drop_item,
        }
    }
}

impl VehicleEntity for BoatEntity {
    fn hurt_time(&self) -> i32 {
        *self.entity_data.lock().vehicle_entity().id_hurt.get()
    }

    fn set_hurt_time(&self, value: i32) {
        self.entity_data
            .lock()
            .vehicle_entity_mut()
            .id_hurt
            .set(value);
    }

    fn hurt_dir(&self) -> i32 {
        *self.entity_data.lock().vehicle_entity().id_hurtdir.get()
    }

    fn set_hurt_dir(&self, value: i32) {
        self.entity_data
            .lock()
            .vehicle_entity_mut()
            .id_hurtdir
            .set(value);
    }

    fn damage(&self) -> f32 {
        *self.entity_data.lock().vehicle_entity().id_damage.get()
    }

    fn set_damage(&self, value: f32) {
        self.entity_data
            .lock()
            .vehicle_entity_mut()
            .id_damage
            .set(value);
    }

    fn vehicle_drop_item(&self) -> ItemRef {
        self.drop_item
    }
}

impl AbstractBoat for BoatEntity {
    fn boat_base(&self) -> &AbstractBoatBase {
        &self.boat_base
    }

    fn set_paddle_state(&self, left: bool, right: bool) {
        let mut data = self.entity_data.lock();
        data.id_paddle_left.set(left);
        data.id_paddle_right.set(right);
    }

    fn raw_paddle_state(&self, side: usize) -> bool {
        let data = self.entity_data.lock();
        *if side == 0 {
            &data.id_paddle_left
        } else {
            &data.id_paddle_right
        }
        .get()
    }

    fn bubble_time(&self) -> i32 {
        *self.entity_data.lock().id_bubble_time.get()
    }

    fn set_bubble_time(&self, time: i32) {
        self.entity_data.lock().id_bubble_time.set(time);
    }

    fn ride_height(&self, dimensions: EntityDimensions) -> f64 {
        f64::from(dimensions.height / 3.0_f32)
    }
}

impl Leashable for BoatEntity {
    fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        self.boat_base.leash_data()
    }
}

impl Entity for BoatEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        let entity = self.into_shared_entity();
        if let Some(boat) = entity.as_abstract_boat() {
            boat.tick_boat(&entity);
        }
    }

    fn get_default_gravity(&self) -> f64 {
        self.boat_default_gravity()
    }

    fn get_air_drag(&self) -> f64 {
        self.boat_air_drag()
    }

    fn is_pickable(&self) -> bool {
        self.boat_is_pickable()
    }

    fn is_pushable(&self) -> bool {
        self.boat_is_pushable()
    }

    fn can_be_collided_with(&self, other: Option<&dyn Entity>) -> bool {
        self.boat_can_be_collided_with(other)
    }

    fn can_collide_with(&self, other: &dyn Entity) -> bool {
        self.boat_can_collide_with(other)
    }

    fn blocks_building(&self) -> bool {
        self.boat_blocks_building()
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        self.boat_movement_emission()
    }

    fn motion_direction(&self) -> Direction {
        self.boat_motion_direction()
    }

    fn dimension_changing_delay(&self) -> i32 {
        self.vehicle_dimension_changing_delay()
    }

    fn get_relative_portal_position(&self, axis: Axis, area: FoundRectangle) -> DVec3 {
        self.boat_relative_portal_position(axis, area)
    }

    fn is_under_water(&self) -> bool {
        self.boat_is_under_water()
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        self.boat_controlling_passenger()
    }

    fn can_add_passenger(&self, passenger: &dyn Entity) -> bool {
        self.boat_can_add_passenger(passenger)
    }

    fn passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        self.boat_passenger_attachment_point(passenger)
    }

    fn position_rider(&self, passenger: &dyn Entity) {
        self.boat_position_rider(passenger);
    }

    fn push_entity(&self, other: &dyn Entity) {
        self.boat_push_entity(other);
    }

    fn on_above_bubble_column(&self, drag_down: bool, _pos: BlockPos) {
        self.boat_on_above_bubble_column(drag_down);
    }

    fn modify_passenger_fluid_interaction_box(
        &self,
        passenger_box: WorldAabb,
    ) -> Option<WorldAabb> {
        self.boat_modify_passenger_fluid_interaction_box(passenger_box)
    }

    fn interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> InteractionResult {
        self.boat_interact(player, hand, location)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.vehicle_hurt(world, source, amount)
    }

    fn check_fall_damage(
        self: Arc<Self>,
        vertical: f64,
        on_ground: bool,
        _state: BlockStateId,
        _pos: BlockPos,
        world: &Arc<World>,
    ) {
        self.boat_check_fall_damage(vertical, on_ground, world);
    }

    fn set_removed(&self, reason: RemovalReason) {
        self.boat_set_removed(reason);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_abstract_boat_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_abstract_boat_data(nbt);
    }

    // TODO: Implement getPickResult.
}
