//! Vehicle entity implementations.

mod boats;
mod minecarts;
mod vehicle_entity;

pub use boats::{AbstractBoat, AbstractBoatBase, BoatEntity, BoatStatus};
pub(crate) use minecarts::create_minecart;
pub use minecarts::{
    AbstractMinecart, AbstractMinecartBase, ChestMinecartEntity, MinecartBehavior, MinecartEntity,
};
pub use vehicle_entity::VehicleEntity;
