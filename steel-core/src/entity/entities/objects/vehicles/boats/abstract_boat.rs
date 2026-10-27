//! Shared boat state and behavior.
use std::f64::consts::PI;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_math::{DEG_TO_RAD, DEGREE_90, DEGREE_270, trig, wrap_degrees};
use steel_registry::TaggedRegistryExt as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::EntityDimensions;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, sound_events, vanilla_blocks, vanilla_game_events};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, Direction, WorldAabb};

use super::super::VehicleEntity;
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext, InteractionResult};
use crate::entity::leash::{LeashData, Leashable};
use crate::entity::{
    Entity, EntityMoveError, EntityMovementEmission, RemovalReason, SharedEntity,
    physics_state_for_move, position_rider_default,
    reset_forward_direction_of_relative_portal_position,
};
use crate::fluid::{get_fluid_state, get_height};
use crate::physics::{CollisionWorld as _, MoverType, WorldCollisionProvider, join_is_not_empty};
use crate::player::Player;
use crate::portal::portal_shape::PortalShape;
use crate::world::World;

const PADDLE_SPEED: f32 = (PI / 8.0) as f32;
const PADDLE_SOUND_TIME: f32 = (PI / 4.0) as f32;
const PADDLE_PERIOD: f32 = (PI * 2.0) as f32;
const TIME_TO_EJECT: f32 = 60.0;
const BUBBLE_TIME: i32 = 60;

/// Boat contact status, in the order used by the water and land checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoatStatus {
    /// Intersecting a water surface.
    InWater,
    /// Fully submerged in source water.
    UnderWater,
    /// Fully submerged in flowing water.
    UnderFlowingWater,
    /// Supported by a colliding block with positive friction.
    OnLand,
    /// Neither in water nor supported by land.
    InAir,
}

#[derive(Debug, Default)]
struct BoatState {
    paddle_positions: [f32; 2],
    out_of_control_ticks: f32,
    delta_rotation: f32,
    water_level: f64,
    land_friction: f32,
    // Java initializes both fields to null; InAir would alter the first tick in water.
    status: Option<BoatStatus>,
    old_status: Option<BoatStatus>,
    last_yd: f64,
    is_above_bubble_column: bool,
    bubble_column_direction_is_down: bool,
}

/// Unsynchronized runtime state shared by boat implementations.
#[derive(Debug, Default)]
pub struct AbstractBoatBase {
    state: SyncMutex<BoatState>,
    leash_data: SyncMutex<Option<LeashData>>,
}

impl AbstractBoatBase {
    /// Returns the leash connection shared by boat implementations.
    pub(super) const fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        &self.leash_data
    }
}

/// Shared server-side state access and behavior for boat implementations.
pub trait AbstractBoat: VehicleEntity + Leashable {
    /// Returns the shared runtime state.
    fn boat_base(&self) -> &AbstractBoatBase;

    /// Sets the two synchronized paddle flags.
    fn set_paddle_state(&self, left: bool, right: bool);

    /// Reads a paddle flag before checking whether a controller exists.
    fn raw_paddle_state(&self, side: usize) -> bool;

    /// Reads the synchronized bubble-column countdown.
    fn bubble_time(&self) -> i32;

    /// Sets the synchronized bubble-column countdown.
    fn set_bubble_time(&self, time: i32);

    /// Returns the passenger attachment height for these dimensions.
    fn ride_height(&self, dimensions: EntityDimensions) -> f64;

    /// Returns the maximum number of direct passengers.
    fn max_passengers(&self) -> usize {
        2
    }

    /// Returns the single passenger's longitudinal offset.
    fn single_passenger_x_offset(&self) -> f32 {
        0.0
    }

    /// Reads an active paddle only while a controlling passenger exists.
    fn paddle_state(&self, side: usize) -> bool {
        self.raw_paddle_state(side) && self.controlling_passenger().is_some()
    }

    /// Returns whether this boat can collide with `other`.
    fn boat_can_collide_with(&self, other: &dyn Entity) -> bool {
        (other.can_be_collided_with(Some(self.as_entity_event_source())) || other.is_pushable())
            && !self.is_passenger_of_same_vehicle(other)
    }

    /// Returns the boat's default gravity.
    fn boat_default_gravity(&self) -> f64 {
        0.04
    }

    /// Returns the boat's air drag.
    fn boat_air_drag(&self) -> f64 {
        1.0
    }

    /// Returns whether this boat can be targeted by picking.
    fn boat_is_pickable(&self) -> bool {
        !self.is_removed()
    }

    /// Returns whether this boat can be pushed.
    fn boat_is_pushable(&self) -> bool {
        true
    }

    /// Returns whether another entity can collide with this boat.
    fn boat_can_be_collided_with(&self, _other: Option<&dyn Entity>) -> bool {
        true
    }

    /// Returns whether this boat obstructs block placement.
    fn boat_blocks_building(&self) -> bool {
        true
    }

    /// Returns the boat's movement emissions.
    fn boat_movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::Events
    }

    /// Returns the boat's direction of motion.
    fn boat_motion_direction(&self) -> Direction {
        self.direction_yaw().rotate_y_clockwise()
    }

    /// Returns this boat's position relative to a portal.
    fn boat_relative_portal_position(&self, axis: Axis, area: FoundRectangle) -> DVec3 {
        reset_forward_direction_of_relative_portal_position(PortalShape::get_relative_position(
            area,
            axis,
            self.position(),
            self.dimensions_for_pose(self.pose()),
        ))
    }

    /// Returns the player controlling this boat, if present.
    fn boat_controlling_passenger(&self) -> Option<SharedEntity> {
        self.first_passenger()
            .filter(|passenger| passenger.as_player().is_some())
    }

    /// Returns whether this boat can accept another passenger.
    fn boat_can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        self.passengers().len() < self.max_passengers() && !self.is_eye_in_water()
    }

    /// Drops a destroyed boat's leash and marks it removed.
    fn boat_set_removed(&self, reason: RemovalReason) {
        if reason.should_destroy() && self.is_leashed() {
            self.drop_leash();
        }
        self.base().set_removed(reason);
    }

    /// Updates fluid and ground-contact caches and returns the contact status.
    fn status(&self, world: &Arc<World>) -> BoatStatus {
        if let Some(status) = self.underwater_status(world) {
            self.boat_base().state.lock().water_level = self.bounding_box().max_y();
            return status;
        }
        if self.check_in_water(world) {
            return BoatStatus::InWater;
        }
        let friction = self.ground_friction(world);
        if friction > 0.0 {
            self.boat_base().state.lock().land_friction = friction;
            BoatStatus::OnLand
        } else {
            BoatStatus::InAir
        }
    }

    /// Classifies full submersion, distinguishing source and flowing water.
    fn underwater_status(&self, world: &Arc<World>) -> Option<BoatStatus> {
        let aabb = self.bounding_box();
        let max_y = aabb.max_y() + 0.001;
        let mut under_water = false;
        for x in aabb.min_x().floor() as i32..aabb.max_x().ceil() as i32 {
            for y in aabb.max_y().floor() as i32..max_y.ceil() as i32 {
                for z in aabb.min_z().floor() as i32..aabb.max_z().ceil() as i32 {
                    let pos = BlockPos::new(x, y, z);
                    let fluid = get_fluid_state(world, pos);
                    if fluid.is_water()
                        && max_y < f64::from(y as f32 + get_height(world, pos, fluid))
                    {
                        if !fluid.is_source() {
                            return Some(BoatStatus::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }
        under_water.then_some(BoatStatus::UnderWater)
    }

    /// Refreshes the maximum water surface intersecting the bottom of the boat.
    fn check_in_water(&self, world: &Arc<World>) -> bool {
        let aabb = self.bounding_box();
        let mut in_water = false;
        let mut water_level = -f64::MAX;
        for x in aabb.min_x().floor() as i32..aabb.max_x().ceil() as i32 {
            for y in aabb.min_y().floor() as i32..(aabb.min_y() + 0.001).ceil() as i32 {
                for z in aabb.min_z().floor() as i32..aabb.max_z().ceil() as i32 {
                    let pos = BlockPos::new(x, y, z);
                    let fluid = get_fluid_state(world, pos);
                    if fluid.is_water() {
                        // The addition is float-valued before Java widens the result.
                        let height = y as f32 + get_height(world, pos, fluid);
                        water_level = f64::from(height).max(water_level);
                        in_water |= aabb.min_y() < f64::from(height);
                    }
                }
            }
        }
        self.boat_base().state.lock().water_level = water_level;
        in_water
    }

    /// Finds the water surface crossed by the previous downward movement.
    fn water_level_above(&self, world: &Arc<World>) -> f32 {
        let aabb = self.bounding_box();
        let last_yd = self.boat_base().state.lock().last_yd;
        let max_y = (aabb.max_y() - last_yd).ceil() as i32;
        let mut pos_y = 0;
        'layers: for y in aabb.max_y().floor() as i32..max_y {
            let mut block_height = 0.0_f32;
            for x in aabb.min_x().floor() as i32..aabb.max_x().ceil() as i32 {
                for z in aabb.min_z().floor() as i32..aabb.max_z().ceil() as i32 {
                    let pos = BlockPos::new(x, y, z);
                    pos_y = y;
                    let fluid = get_fluid_state(world, pos);
                    if fluid.is_water() {
                        block_height = block_height.max(get_height(world, pos, fluid));
                    }
                    if block_height >= 1.0 {
                        continue 'layers;
                    }
                }
            }
            if block_height < 1.0 {
                return pos_y as f32 + block_height;
            }
        }
        max_y.wrapping_add(1) as f32
    }

    /// Averages friction over supporting collision shapes, excluding lily pads.
    fn ground_friction(&self, world: &Arc<World>) -> f32 {
        let aabb = self.bounding_box();
        let footprint = WorldAabb::new(
            aabb.min_x(),
            aabb.min_y() - 0.001,
            aabb.min_z(),
            aabb.max_x(),
            aabb.min_y(),
            aabb.max_z(),
        );
        let x0 = footprint.min_x().floor() as i32 - 1;
        let x1 = footprint.max_x().ceil() as i32 + 1;
        let y0 = footprint.min_y().floor() as i32 - 1;
        let y1 = footprint.max_y().ceil() as i32 + 1;
        let z0 = footprint.min_z().floor() as i32 - 1;
        let z1 = footprint.max_z().ceil() as i32 + 1;
        let mut friction = 0.0_f32;
        let mut count = 0_i32;
        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges == 2 {
                    continue;
                }
                for y in y0..y1 {
                    if edges > 0 && (y == y0 || y == y1 - 1) {
                        continue;
                    }
                    let pos = BlockPos::new(x, y, z);
                    let state = world.get_block_state(pos);
                    let block = state.get_block();
                    let behavior = BLOCK_BEHAVIORS.get_behavior(block);
                    if block == &vanilla_blocks::LILY_PAD {
                        continue;
                    }
                    if behavior
                        .get_collision_boxes(
                            state,
                            world.as_ref(),
                            pos,
                            BlockCollisionContext::empty(),
                        )
                        .iter()
                        .any(|shape| join_is_not_empty(&shape.at_block(pos), &footprint))
                    {
                        friction += block.config.friction;
                        count += 1;
                    }
                }
            }
        }
        // No supporting cells deliberately yields NaN, which fails the status's > 0 test.
        friction / count as f32
    }

    /// Applies drag and buoyancy, including the air-to-water surface transition.
    fn float_boat(&self, world: &Arc<World>) -> Result<(), EntityMoveError> {
        let mut vertical_speed = -self.get_gravity();
        let mut buoyancy = 0.0_f64;
        let mut inverse_friction = 0.05_f32;
        let (old_status, status, water_level, land_friction) = {
            let state = self.boat_base().state.lock();
            (
                state.old_status,
                state.status,
                state.water_level,
                state.land_friction,
            )
        };
        if old_status == Some(BoatStatus::InAir)
            && status != Some(BoatStatus::InAir)
            && status != Some(BoatStatus::OnLand)
        {
            let position = self.position();
            let height = self.base().dimensions().height;
            self.boat_base().state.lock().water_level = position.y + f64::from(height) * 1.0;
            let target_y = f64::from(self.water_level_above(world) - height) + 0.101;
            let target_box =
                self.bounding_box()
                    .translate(DVec3::new(0.0, target_y - position.y, 0.0));
            let collisions =
                WorldCollisionProvider::for_entity(world, self.as_entity_event_source());
            let context =
                physics_state_for_move(self.as_entity_event_source()).block_collision_context();
            if !collisions.has_collision_with_context(&target_box, context) {
                self.try_set_position(DVec3::new(position.x, target_y, position.z))?;
                let movement = self.velocity();
                self.set_velocity(DVec3::new(
                    movement.x * 1.0,
                    movement.y * 0.0,
                    movement.z * 1.0,
                ));
                self.boat_base().state.lock().last_yd = 0.0;
            }
            self.boat_base().state.lock().status = Some(BoatStatus::InWater);
            return Ok(());
        }
        match status {
            Some(BoatStatus::InWater) => {
                buoyancy =
                    (water_level - self.position().y) / f64::from(self.base().dimensions().height);
                inverse_friction = 0.9;
            }
            Some(BoatStatus::UnderFlowingWater) => {
                vertical_speed = -7.0e-4;
                inverse_friction = 0.9;
            }
            Some(BoatStatus::UnderWater) => {
                buoyancy = f64::from(0.01_f32);
                inverse_friction = 0.45;
            }
            Some(BoatStatus::InAir) => inverse_friction = 0.9,
            Some(BoatStatus::OnLand) => {
                inverse_friction = land_friction;
                if self
                    .controlling_passenger()
                    .is_some_and(|passenger| passenger.as_player().is_some())
                {
                    self.boat_base().state.lock().land_friction /= 2.0_f32;
                }
            }
            None => {}
        }
        let movement = self.velocity();
        self.set_velocity(DVec3::new(
            movement.x * f64::from(inverse_friction),
            movement.y + vertical_speed,
            movement.z * f64::from(inverse_friction),
        ));
        self.boat_base().state.lock().delta_rotation *= inverse_friction;
        if buoyancy > 0.0 {
            let movement = self.velocity();
            self.set_velocity(DVec3::new(
                movement.x,
                (movement.y + buoyancy * (self.get_default_gravity() / 0.65)) * 0.75,
                movement.z,
            ));
        }
        Ok(())
    }

    /// Tracks previous vertical velocity and the boat's fall-distance state.
    fn boat_check_fall_damage(&self, vertical: f64, on_ground: bool, world: &Arc<World>) {
        self.boat_base().state.lock().last_yd = self.velocity().y;
        if self.is_passenger() {
            return;
        }
        if on_ground {
            self.reset_fall_distance();
        } else if !get_fluid_state(world, self.block_position().below()).is_water()
            && vertical < 0.0
        {
            self.set_fall_distance(self.fall_distance() - f64::from(vertical as f32));
        }
    }

    /// Selects a passenger seat and rotates its attachment by the boat's yaw.
    fn boat_passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        let passengers = self.passengers();
        let mut offset = self.single_passenger_x_offset();
        if passengers.len() > 1 {
            offset = if passengers
                .first()
                .is_some_and(|first| first.id() == passenger.id())
            {
                0.2_f32
            } else {
                -0.6_f32
            };
            if passenger.is_animal() {
                offset += 0.2_f32;
            }
        }
        let angle = f64::from(-self.rotation().0 * DEG_TO_RAD);
        let cos = f64::from(trig::cos(angle));
        let sin = f64::from(trig::sin(angle));
        let offset = f64::from(offset);
        DVec3::new(
            0.0 * cos + offset * sin,
            self.ride_height(self.base().dimensions()),
            offset * cos - 0.0 * sin,
        )
    }

    /// Returns whether the entity is narrow enough to board this boat.
    fn has_enough_space_for(&self, entity: &dyn Entity) -> bool {
        entity.base().dimensions().width < self.base().dimensions().width
    }

    /// Positions a passenger and constrains its body and head rotation.
    fn boat_position_rider(&self, passenger: &dyn Entity) {
        position_rider_default(self, passenger);
        if REGISTRY
            .entity_types
            .is_in_tag(passenger.entity_type(), &EntityTypeTag::CAN_TURN_IN_BOATS)
        {
            return;
        }
        if passenger.is_server_driven_movement() {
            let delta = self.boat_base().state.lock().delta_rotation;
            passenger.set_y_rot(passenger.rotation().0 + delta);
            if let Some(living) = passenger.as_living_entity() {
                living.set_y_head_rot(living.y_head_rot() + delta);
            }
        }
        self.clamp_rotation(passenger);
    }

    /// Calculates the passenger's body yaw for its boat seat.
    fn calculate_passenger_body_y_rot(&self, passenger: &dyn Entity) -> f32 {
        if passenger.is_animal() && self.passengers().len() == self.max_passengers() {
            self.rotation().0
                + if passenger.id() % 2 == 0 {
                    DEGREE_90
                } else {
                    DEGREE_270
                }
        } else {
            self.rotation().0
        }
    }

    /// Applies the passenger's allowed yaw range and returns the yaw adjustment.
    fn clamp_rotation(&self, passenger: &dyn Entity) -> f32 {
        let body_yaw = self.calculate_passenger_body_y_rot(passenger);
        if let Some(living) = passenger.as_living_entity() {
            living.set_y_body_rot(body_yaw);
        }
        if let Some(mob) = passenger.as_mob() {
            passenger.set_y_rot(body_yaw);
            mob.clamp_head_rotation_to_body();
            return wrap_degrees(body_yaw - passenger.rotation().0);
        }
        let delta = wrap_degrees(passenger.rotation().0 - body_yaw);
        let target_delta = delta.clamp(-105.0_f32, 105.0_f32);
        passenger.set_y_rot(passenger.rotation().0 + target_delta - delta);
        target_delta - delta
    }

    /// Applies separation when the other entity overlaps the allowed vertical range.
    fn boat_push_entity(&self, other: &dyn Entity) {
        let collides = if other.entity_type().is_abstract_boat {
            other.bounding_box().min_y() < self.bounding_box().max_y()
        } else {
            other.bounding_box().min_y() <= self.bounding_box().min_y()
        };
        if collides {
            self.default_push_entity(other);
        }
    }

    /// Clips a rider's fluid contact above a hull that is not fully submerged.
    fn boat_modify_passenger_fluid_interaction_box(
        &self,
        passenger_box: WorldAabb,
    ) -> Option<WorldAabb> {
        if self.is_under_water() {
            return Some(passenger_box);
        }
        let boat_box = self.bounding_box();
        if boat_box.max_y() >= passenger_box.max_y() {
            return None;
        }
        Some(WorldAabb::new(
            passenger_box.min_x(),
            passenger_box.min_y().max(boat_box.max_y()),
            passenger_box.min_z(),
            passenger_box.max_x(),
            passenger_box.max_y(),
            passenger_box.max_z(),
        ))
    }

    /// Advances movement, paddles, bubble effects and nearby entity boarding.
    fn tick_boat(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        {
            let mut state = self.boat_base().state.lock();
            state.old_status = state.status;
        }
        let status = self.status(&world);
        let out_of_control_ticks = {
            let mut state = self.boat_base().state.lock();
            state.status = Some(status);
            if matches!(
                status,
                BoatStatus::UnderWater | BoatStatus::UnderFlowingWater
            ) {
                state.out_of_control_ticks += 1.0_f32;
            } else {
                state.out_of_control_ticks = 0.0;
            }
            state.out_of_control_ticks
        };
        if out_of_control_ticks >= TIME_TO_EJECT {
            self.eject_passengers();
        }
        if self.hurt_time() > 0 {
            self.set_hurt_time(self.hurt_time() - 1);
        }
        if self.damage() > 0.0 {
            self.set_damage(self.damage() - 1.0_f32);
        }
        self.default_tick();
        if self.is_server_driven_movement() {
            if self
                .first_passenger()
                .is_none_or(|passenger| passenger.as_player().is_none())
            {
                self.set_paddle_state(false, false);
            }
            if let Err(error) = self.float_boat(&world) {
                log::debug!("Rejected boat {} water-entry movement: {error}", self.id());
                return;
            }
            if Arc::clone(entity)
                .move_entity(MoverType::SelfMovement, self.velocity())
                .is_none()
            {
                return;
            }
        } else {
            self.set_velocity(DVec3::ZERO);
            self.boat_base().state.lock().delta_rotation = 0.0;
        }
        self.apply_effects_from_blocks();
        self.apply_effects_from_blocks();
        self.tick_bubble_column();
        self.tick_paddles(&world);
        self.push_and_pick_up_entities(entity, &world);
    }

    /// Records surface bubble contact and starts its countdown.
    fn boat_on_above_bubble_column(&self, drag_down: bool) {
        {
            let mut state = self.boat_base().state.lock();
            state.is_above_bubble_column = true;
            state.bubble_column_direction_is_down = drag_down;
        }
        if self.bubble_time() == 0 {
            self.set_bubble_time(BUBBLE_TIME);
        }
        if !self.is_under_water() && rand::random_range(0..100) == 0 {
            // The splash sound and particle are client-local; the game event is server-side.
            let controller = self.controlling_passenger();
            self.game_event_with_source_entity(&vanilla_game_events::SPLASH, controller.as_deref());
        }
    }

    /// Tracks continuous bubble contact and launches or ejects at expiry.
    fn tick_bubble_column(&self) {
        if !self.boat_base().state.lock().is_above_bubble_column {
            self.set_bubble_time(0);
        }
        let time = self.bubble_time();
        if time <= 0 {
            return;
        }
        let time = time - 1;
        self.set_bubble_time(time);
        let diff = BUBBLE_TIME.wrapping_sub(time).wrapping_sub(1);
        if diff > 0 && time == 0 {
            self.set_bubble_time(0);
            let drag_down = self
                .boat_base()
                .state
                .lock()
                .bubble_column_direction_is_down;
            if drag_down {
                self.eject_passengers();
                self.broadcast_entity_event(EntityStatus::BoatSink);
            } else {
                self.broadcast_entity_event(EntityStatus::BoatLaunch);
            }

            self.handle_bubble_column_effect(drag_down);
        }
        self.boat_base().state.lock().is_above_bubble_column = false;
    }

    /// Applies the boat's sinking or launching motion from a bubble column.
    fn handle_bubble_column_effect(&self, drag_down: bool) {
        if !self.can_simulate_movement() {
            return;
        }

        let movement = self.velocity();
        self.set_velocity(if drag_down {
            movement + DVec3::new(0.0, -0.7, 0.0)
        } else {
            DVec3::new(movement.x, 0.6, movement.z)
        });
    }

    /// Selects the paddle sound for the boat's current surroundings.
    fn get_paddle_sound(&self, world: &Arc<World>) -> Option<SoundEventRef> {
        match self.status(world) {
            BoatStatus::InWater | BoatStatus::UnderWater | BoatStatus::UnderFlowingWater => {
                Some(&sound_events::ENTITY_BOAT_PADDLE_WATER)
            }
            BoatStatus::OnLand => Some(&sound_events::ENTITY_BOAT_PADDLE_LAND),
            BoatStatus::InAir => None,
        }
    }

    /// Advances paddle phases and emits sounds when a stroke crosses its sound angle.
    fn tick_paddles(&self, world: &Arc<World>) {
        for side in 0..=1 {
            if !self.paddle_state(side) {
                self.boat_base().state.lock().paddle_positions[side] = 0.0;
                continue;
            }
            let phase = self.boat_base().state.lock().paddle_positions[side];
            if !self.is_silent()
                && phase % PADDLE_PERIOD <= PADDLE_SOUND_TIME
                && (phase + PADDLE_SPEED) % PADDLE_PERIOD >= PADDLE_SOUND_TIME
            {
                if let Some(sound) = self.get_paddle_sound(world) {
                    let view = self.look_angle();
                    let dx = if side == 1 { -view.z } else { view.z };
                    let dz = if side == 1 { view.x } else { -view.x };
                    world.play_sound_at(
                        sound,
                        self.sound_source(),
                        self.position() + DVec3::new(dx, 0.0, dz),
                        1.0,
                        0.8_f32 + 0.4_f32 * rand::random::<f32>(),
                        None,
                    );
                }
            }
            self.boat_base().state.lock().paddle_positions[side] = phase + PADDLE_SPEED;
        }
    }

    /// Boards eligible nearby living entities or applies boat push filtering.
    fn push_and_pick_up_entities(&self, boat_entity: &SharedEntity, world: &World) {
        let aabb = self.bounding_box().inflate_xyz(
            f64::from(0.2_f32),
            f64::from(-0.01_f32),
            f64::from(0.2_f32),
        );
        let entities = world.get_pushable_entities(self.as_entity_event_source(), &aabb);
        if entities.is_empty() {
            return;
        }
        let add_new_passengers = self
            .controlling_passenger()
            .is_none_or(|passenger| passenger.as_player().is_none());

        for entity in entities {
            if entity.has_passenger(self.as_entity_event_source()) {
                continue;
            }
            if add_new_passengers
                && self.passengers().len() < self.max_passengers()
                && !entity.is_passenger()
                && self.has_enough_space_for(entity.as_ref())
                && entity.is_living_entity()
                && !REGISTRY.entity_types.is_in_tag(
                    entity.entity_type(),
                    &EntityTypeTag::CANNOT_BE_PUSHED_ONTO_BOATS,
                )
            {
                let _ = entity.start_riding(boat_entity);
            } else {
                self.push_entity(entity.as_ref());
            }
        }
    }

    /// Reports full submersion from the cached contact classification.
    fn boat_is_under_water(&self) -> bool {
        matches!(
            self.boat_base().state.lock().status,
            Some(BoatStatus::UnderWater | BoatStatus::UnderFlowingWater)
        )
    }

    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "preserves Java's NaN interaction branch"
    )]
    /// Handles shared leash interactions before attempting player boarding.
    fn boat_interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> InteractionResult {
        let result = self.interact_entity(player, hand, location);
        if result != InteractionResult::Pass {
            return result;
        }
        if player.is_secondary_use_active()
            || !(self.boat_base().state.lock().out_of_control_ticks < TIME_TO_EJECT)
        {
            return InteractionResult::Pass;
        }
        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        let Some(boat) = world.get_entity_by_id(self.id()) else {
            return InteractionResult::Pass;
        };
        if player.start_riding(&boat) {
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }

    /// Writes the boat's persisted leash connection.
    fn save_abstract_boat_data(&self, nbt: &mut NbtCompound) {
        if let Some(leash) = self.leash_data().lock().as_ref() {
            leash.save(nbt);
        }
    }

    /// Restores the boat's saved leash connection.
    fn load_abstract_boat_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        *self.leash_data().lock() = LeashData::load(nbt);
    }
}

#[cfg(test)]
#[path = "tests/abstract_boat.rs"]
mod tests;
