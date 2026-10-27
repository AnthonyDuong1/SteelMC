//! Boat entity implementations.

mod abstract_boat;
mod boat;

pub use abstract_boat::{AbstractBoat, AbstractBoatBase, BoatStatus};
pub use boat::BoatEntity;
