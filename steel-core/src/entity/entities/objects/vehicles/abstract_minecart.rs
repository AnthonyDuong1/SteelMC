//! Shared vanilla `AbstractMinecart` state and behavior.
//!
//! `AbstractMinecartState` holds the two fields vanilla never networks
//! (`onRails`, `flipped`). The trait is named `AbstractMinecart` to match
//! the vanilla class it ports — same convention as the `Entity` trait —
//! now that the struct doesn't need that name too.

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_math::trig;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::RailShape;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{vanilla_blocks, vanilla_entities};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId};

use super::minecart_behavior::MinecartBehavior;
use super::vehicle_entity::VehicleEntity;
use crate::block_entity::block_state_nbt;
use crate::entity::reset_forward_direction_of_relative_portal_position;
use crate::entity::{Entity, EntityMovementEmission};
use crate::physics::{MoveResult, MoverType};
use crate::portal::portal_shape::PortalShape;
use crate::world::{SignalGetter as _, World};

const LOWERED_PASSENGER_ATTACHMENT: DVec3 = DVec3::ZERO;
const AIR_DRAG: f32 = 0.95;
const WATER_SLOWDOWN_FACTOR: f32 = 0.95;
const MIN_PUSH_DISTANCE_SQUARED: f32 = 1.0e-4;
const MIN_COLLISION_ALIGNMENT: f32 = 0.8;
const GRAVITY_IN_WATER: f64 = 0.005;
const DEFAULT_GRAVITY: f64 = 0.04;

/// The two vanilla `AbstractMinecart` fields with no `SynchedEntityData`
/// backing (`onRails`, `flipped`).
#[derive(Debug, Default)]
pub struct AbstractMinecartBase {
    state: SyncMutex<AbstractMinecartState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AbstractMinecartState {
    on_rails: bool,
    flipped: bool,
}

impl AbstractMinecartBase {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn on_rails(&self) -> bool {
        self.state.lock().on_rails
    }

    pub(super) fn set_on_rails(&self, value: bool) {
        self.state.lock().on_rails = value;
    }

    fn flipped(&self) -> bool {
        self.state.lock().flipped
    }

    pub(super) fn set_flipped(&self, flipped: bool) {
        self.state.lock().flipped = flipped;
    }
}

/// Object-safe access to an abstract minecart trait object from default [`AbstractMinecart`] methods.
pub trait AbstractMinecartEventSource {
    /// Returns this entity as an abstract minecart.
    fn as_abstract_minecart_event_source(&self) -> &dyn AbstractMinecart;
}

impl<T: AbstractMinecart> AbstractMinecartEventSource for T {
    fn as_abstract_minecart_event_source(&self) -> &dyn AbstractMinecart {
        self
    }
}

/// Shared behavior for every minecart variant entity.
pub trait AbstractMinecart: VehicleEntity + AbstractMinecartEventSource {
    /// Returns the minecart's shared non-synchronized minecart state.
    fn minecart_base(&self) -> &AbstractMinecartBase;

    /// Returns the movement behavior used by this minecart.
    fn minecart_behavior(&self) -> &dyn MinecartBehavior;

    /// Returns the custom display block, if any.
    fn custom_display_block(&self) -> Option<BlockStateId>;

    /// Sets the custom display block.
    fn set_custom_display_block(&self, value: Option<BlockStateId>);

    /// Mirrors `AbstractMinecart.getDefaultDisplayOffset()`.
    fn default_display_offset(&self) -> i32 {
        6
    }

    /// Returns the default gravity of this minecart.
    fn minecart_default_gravity(&self) -> f64 {
        if self.is_in_water() {
            GRAVITY_IN_WATER
        } else {
            DEFAULT_GRAVITY
        }
    }

    /// Returns whether this minecart can be targeted by picking.
    fn minecart_is_pickable(&self) -> bool {
        !self.is_removed()
    }

    /// Returns whether this minecart can be pushed.
    fn minecart_is_pushable(&self) -> bool {
        true
    }

    /// Returns the movement side effects emitted by this minecart.
    fn minecart_movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::Events
    }

    /// Returns whether this minecart obstructs block placement.
    fn minecart_blocks_building(&self) -> bool {
        true
    }

    /// Returns the vertical offset of the minecart's display block.
    fn display_offset(&self) -> i32;

    /// Sets the vertical offset of the minecart's display block.
    fn set_display_offset(&self, value: i32);

    /// Mirrors `AbstractMinecart.getDisplayBlockState`.
    fn display_block_state(&self) -> BlockStateId {
        self.custom_display_block()
            .unwrap_or_else(|| self.default_display_block_state())
    }

    /// Mirrors `AbstractMinecart.getDefaultDisplayBlockState`.
    fn default_display_block_state(&self) -> BlockStateId {
        vanilla_blocks::AIR.default_state()
    }

    /// Mirrors `AbstractMinecart.activateMinecart`. Default does nothing.
    fn on_activator_rail(&self, _world: &World, _pos: BlockPos, _powered: bool) {}

    /// Returns whether this minecart is rideable.
    fn is_rideable(&self) -> bool {
        false
    }

    /// Returns whether this minecart's movement-facing rotation is flipped.
    fn is_flipped(&self) -> bool {
        self.minecart_base().flipped()
    }

    /// Returns whether the block at `pos` is a redstone conductor.
    fn is_redstone_conductor(&self, world: &World, pos: BlockPos) -> bool {
        let state = world.get_block_state(pos);
        world.is_redstone_conductor(state, pos)
    }

    /// Returns whether this minecart is a furnace minecart.
    fn is_furnace(&self) -> bool {
        false
    }

    /// Returns whether this minecart can collide with `other`.
    fn minecart_can_collide_with(&self, other: &dyn Entity) -> bool {
        (other.can_be_collided_with(Some(self.as_entity_event_source())) || other.is_pushable())
            && !self.is_passenger_of_same_vehicle(other)
    }

    /// Mirrors `AbstractMinecart.getCurrentBlockPosOrRailBelow`.
    fn current_block_pos_or_rail_below(&self, world: &World) -> BlockPos {
        let pos = self.position();
        let (xt, yt, zt) = (
            pos.x.floor() as i32,
            pos.y.floor() as i32,
            pos.z.floor() as i32,
        );
        let below = BlockPos::new(xt, yt - 1, zt);
        if world
            .get_block_state(below)
            .get_block()
            .has_tag(&BlockTag::RAILS)
        {
            below
        } else {
            BlockPos::new(xt, yt, zt)
        }
    }

    /// Mirrors `AbstractMinecart.getBlockSpeedFactor()`.
    fn minecart_block_speed_factor(&self) -> f32 {
        let Some(world) = self.level() else {
            return 1.0;
        };

        let state = world.get_block_state(self.block_position());
        if state.get_block().has_tag(&BlockTag::RAILS) {
            1.0
        } else {
            self.default_block_speed_factor()
        }
    }

    /// Mirrors `AbstractMinecart.applyMinecartEffectsFromBlocks()` behavior.
    fn minecart_apply_effects_from_blocks(&self) {
        let position = self.position();
        self.apply_effects_from_blocks_between(position, position);
        self.base().clear_movement_this_tick();
    }

    /// Moves this minecart using default entity movement, then applies block-contact effects.
    fn minecart_move_entity(&self, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        let result = self.default_move_entity(mover_type, delta);
        self.minecart_apply_effects_from_blocks();
        result
    }

    /// Mirrors `AbstractMinecart.tick()`'s server-relevant portion.
    fn tick_minecart(&self) {
        let Some(world) = self.level() else {
            return;
        };

        if self.hurt_time() > 0 {
            self.set_hurt_time(self.hurt_time() - 1);
        }
        if self.damage() > 0.0 {
            self.set_damage(self.damage() - 1.0);
        }

        self.check_below_world();
        self.base().compute_known_speed();
        self.handle_portal();
        self.minecart_behavior()
            .tick(self.as_abstract_minecart_event_source(), &world);
        self.refresh_fluid_contact_for_base_tick();

        if self.is_in_lava() {
            self.lava_ignite();
            self.lava_hurt();
            self.set_fall_distance(self.fall_distance() * 0.5);
        }

        // TODO: set first tick to false
    }

    /// Applies natural slowdown to the minecart movement.
    fn apply_natural_slowdown(&self, movement: DVec3) -> DVec3 {
        let slowdown = self
            .minecart_behavior()
            .slowdown_factor(self.as_abstract_minecart_event_source());
        let mut result = movement * DVec3::new(slowdown, 0.0, slowdown);
        if self.is_in_water() {
            result *= f64::from(WATER_SLOWDOWN_FACTOR);
        }
        result
    }

    /// Slows down the minecart when it comes off the track.
    fn come_off_track(&self) {
        let behavior = self.minecart_behavior();
        let max_speed = behavior.max_speed(self.as_abstract_minecart_event_source());
        let velocity = self.velocity();
        let mut velocity = DVec3::new(
            velocity.x.clamp(-max_speed, max_speed),
            velocity.y,
            velocity.z.clamp(-max_speed, max_speed),
        );
        if self.on_ground() {
            velocity *= 0.5;
        }
        self.set_velocity(velocity);
        if self
            .move_entity(MoverType::SelfMovement, self.velocity())
            .is_none()
        {
            return;
        }

        if !self.on_ground() {
            self.set_velocity(self.velocity() * f64::from(AIR_DRAG));
        }
    }

    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "negated comparisons preserve Vanilla's NaN branch behavior"
    )]
    /// Mirrors `AbstractMinecart.push(Entity)`.
    fn minecart_push_entity(&self, other: &dyn Entity) {
        if other.no_physics() || self.no_physics() || self.has_passenger(other) {
            return;
        }

        let mut xa = other.position().x - self.position().x;
        let mut za = other.position().z - self.position().z;
        let dd = xa * xa + za * za;
        if !(dd >= f64::from(MIN_PUSH_DISTANCE_SQUARED)) {
            return;
        }

        let dd = dd.sqrt();
        xa /= dd;
        za /= dd;
        let pow = (1.0 / dd).min(1.0);
        xa *= pow * 0.1 * 0.5;
        za *= pow * 0.1 * 0.5;

        if let Some(other_minecart) = other.as_abstract_minecart() {
            self.push_other_minecart(other_minecart, xa, za);
        } else {
            self.push_impulse(DVec3::new(-xa, 0.0, -za));
            other.push_impulse(DVec3::new(xa / 4.0, 0.0, za / 4.0));
        }
    }

    /// Mirrors `AbstractMinecart.pushOtherMinecart`.
    ///
    /// Steel has no experimental-movement feature flag yet, so this always
    /// takes vanilla's non-experimental branch: direction comes from relative
    /// position rather than this minecart's own velocity.
    fn push_other_minecart(&self, other_minecart: &dyn AbstractMinecart, xa: f64, za: f64) {
        let xo = other_minecart.position().x - self.position().x;
        let zo = other_minecart.position().z - self.position().z;

        let dir = DVec3::new(xo, 0.0, zo).normalize_or_zero();
        let yaw = f64::from(self.rotation().0.to_radians());
        let facing = DVec3::new(f64::from(trig::cos(yaw)), 0.0, f64::from(trig::sin(yaw)))
            .normalize_or_zero();
        let dot = dir.dot(facing).abs();

        if dot < f64::from(MIN_COLLISION_ALIGNMENT) {
            return;
        }

        let movement = self.velocity();
        let other_movement = other_minecart.velocity();

        if other_minecart.is_furnace() && !self.is_furnace() {
            self.set_velocity(movement * DVec3::new(0.2, 1.0, 0.2));
            self.push_impulse(DVec3::new(
                other_movement.x - xa,
                0.0,
                other_movement.z - za,
            ));
            other_minecart.set_velocity(other_movement * DVec3::new(0.95, 1.0, 0.95));
        } else if !other_minecart.is_furnace() && self.is_furnace() {
            other_minecart.set_velocity(other_movement * DVec3::new(0.2, 1.0, 0.2));
            other_minecart.push_impulse(DVec3::new(movement.x + xa, 0.0, movement.z + za));
            self.set_velocity(movement * DVec3::new(0.95, 1.0, 0.95));
        } else {
            let xdd = f64::midpoint(other_movement.x, movement.x);
            let zdd = f64::midpoint(other_movement.z, movement.z);
            self.set_velocity(movement * DVec3::new(0.2, 1.0, 0.2));
            self.push_impulse(DVec3::new(xdd - xa, 0.0, zdd - za));
            other_minecart.set_velocity(other_movement * DVec3::new(0.2, 1.0, 0.2));
            other_minecart.push_impulse(DVec3::new(xdd + xa, 0.0, zdd + za));
        }
    }

    /// Mirrors `AbstractMinecart.getRelativePortalPosition`.
    fn minecart_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        reset_forward_direction_of_relative_portal_position(PortalShape::get_relative_position(
            portal_area,
            axis,
            self.position(),
            self.dimensions_for_pose(self.pose()),
        ))
    }

    /// Returns the passenger attachment point used by this minecart.
    ///
    /// Villagers and wandering traders use vanilla's lowered attachment point.
    fn minecart_passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        let passenger_type = passenger.entity_type();

        if passenger_type == &vanilla_entities::VILLAGER
            || passenger_type == &vanilla_entities::WANDERING_TRADER
        {
            LOWERED_PASSENGER_ATTACHMENT
        } else {
            self.default_passenger_attachment_point(passenger)
        }
    }

    /// Mirrors `AbstractMinecart.addAdditionalSaveData`'s base-class portion.
    fn save_abstract_minecart_data(&self, nbt: &mut NbtCompound) {
        if let Some(display_state) = self.custom_display_block() {
            nbt.insert(
                "DisplayState",
                NbtTag::Compound(block_state_nbt::save(display_state)),
            );
        }
        if self.display_offset() != self.default_display_offset() {
            nbt.insert("DisplayOffset", self.display_offset());
        }
        nbt.insert("FlippedRotation", i8::from(self.minecart_base().flipped()));
    }

    /// Mirrors `AbstractMinecart.readAdditionalSaveData`'s base-class portion.
    fn load_abstract_minecart_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_custom_display_block(nbt.compound("DisplayState").and_then(block_state_nbt::load));
        self.set_display_offset(
            nbt.int("DisplayOffset")
                .unwrap_or_else(|| self.default_display_offset()),
        );

        let flipped = nbt.byte("FlippedRotation").unwrap_or(0) != 0;
        self.minecart_base().set_flipped(flipped);
    }
}

/// Mirrors `AbstractMinecart.EXITS` / `AbstractMinecart.exits(RailShape)`.
#[must_use]
pub const fn rail_exits(shape: RailShape) -> ((i32, i32, i32), (i32, i32, i32)) {
    let (x_neg, x_pos, z_neg, z_pos) = ((-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1));
    let (x_neg_below, x_pos_below, z_neg_below, z_pos_below) =
        ((-1, -1, 0), (1, -1, 0), (0, -1, -1), (0, -1, 1));
    match shape {
        RailShape::NorthSouth => (z_neg, z_pos),
        RailShape::EastWest => (x_neg, x_pos),
        RailShape::AscendingEast => (x_neg_below, x_pos),
        RailShape::AscendingWest => (x_neg, x_pos_below),
        RailShape::AscendingNorth => (z_neg, z_pos_below),
        RailShape::AscendingSouth => (z_neg_below, z_pos),
        RailShape::SouthEast => (z_pos, x_pos),
        RailShape::SouthWest => (z_pos, x_neg),
        RailShape::NorthWest => (z_neg, x_neg),
        RailShape::NorthEast => (z_neg, x_pos),
    }
}
