use glam::DVec3;
use steel_utils::Direction;

use super::abstract_minecart::AbstractMinecart;
use crate::world::World;

/// A minecart movement strategy — vanilla `MinecartBehavior`.
///
/// `OldMinecartBehavior` is the only implementation today.
/// TODO(minecart-improvements): add `NewMinecartBehavior` once Steel can
/// select it using the corresponding world feature flag.
pub trait MinecartBehavior: Send + Sync {
    /// Advances this minecart's movement behavior by one game tick.
    fn tick(&self, minecart: &dyn AbstractMinecart, world: &World);

    /// Moves the minecart along its current rail.
    fn move_along_track(&self, minecart: &dyn AbstractMinecart, world: &World);

    /// Returns whether the minecart should continue its movement after pushing or picking up entities.
    fn push_and_pickup_entities(&self, minecart: &dyn AbstractMinecart) -> bool;

    /// Returns the horizontal direction used by this minecart's movement behavior.
    fn motion_direction(&self, minecart: &dyn AbstractMinecart) -> Direction {
        minecart.direction_yaw()
    }

    /// Returns the minecart movement used when processing block contacts.
    fn known_movement(&self, known_movement: DVec3) -> DVec3 {
        known_movement
    }

    /// Returns the maximum speed of this minecart.
    fn max_speed(&self, minecart: &dyn AbstractMinecart) -> f64;

    /// Returns the slowdown factor of this minecart.
    fn slowdown_factor(&self, minecart: &dyn AbstractMinecart) -> f64;
}
