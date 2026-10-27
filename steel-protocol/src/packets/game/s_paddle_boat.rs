use steel_macros::{ReadFrom, ServerPacket};

/// Serverbound boat paddle state.
#[derive(ReadFrom, Clone, Debug, ServerPacket)]
pub struct SPaddleBoat {
    /// Whether the left paddle is active.
    pub left: bool,
    /// Whether the right paddle is active.
    pub right: bool,
}
