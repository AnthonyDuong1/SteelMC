use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;

use crate::behavior::BlockHitResult;
use crate::behavior::item_utils::get_player_pov_hit_result;
use crate::behavior::{BlockCollisionContext, InteractionResult, ItemBehavior, UseItemContext};
use crate::entity::{
    ENTITIES, Entity, SharedEntity, add_spawned_entity, apply_item_stack_components,
    create_entity_instance,
};
use crate::physics::{CollisionWorld as _, WorldCollisionProvider};
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{ClipFluid, World};

/// Places boat types whose entity implementation is available.
#[item_behavior(class = "BoatItem")]
pub struct BoatItem {
    #[json_arg(vanilla_entities, json = "entity_type")]
    entity_type: EntityTypeRef,
}

impl BoatItem {
    /// Creates a behavior using the extracted entity type.
    #[must_use]
    pub const fn new(entity_type: EntityTypeRef) -> Self {
        Self { entity_type }
    }

    fn get_boat(
        &self,
        world: &Arc<World>,
        hit: &BlockHitResult,
        stack: &ItemStack,
        player: &Player,
    ) -> Option<SharedEntity> {
        let boat = match create_entity_instance(world, self.entity_type, hit.location) {
            Ok(boat) => boat,
            Err(error) => {
                log::warn!(
                    "Failed to create boat {} at {:?}: {error:?}",
                    self.entity_type.key,
                    hit.location,
                );
                return None;
            }
        };

        if boat.as_abstract_boat().is_none() {
            log::error!(
                "Entity factory for {} does not implement AbstractBoat",
                self.entity_type.key,
            );
            return None;
        }

        boat.set_old_position_to_current();

        if let Err(error) = apply_item_stack_components(&boat, stack, player.is_operator()) {
            log::warn!(
                "Failed to configure boat {} from its item: {error:?}",
                self.entity_type.key,
            );
            return None;
        }

        Some(boat)
    }
}

impl ItemBehavior for BoatItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Chest boats and rafts retain their existing behavior until their factories exist.
        if !ENTITIES.has_factory(self.entity_type) {
            return InteractionResult::Pass;
        }
        let hit = get_player_pov_hit_result(context.world, context.player, ClipFluid::Any);
        if hit.miss {
            return InteractionResult::Pass;
        }
        let player = context.player;
        let position = player.position();
        let eye = DVec3::new(position.x, player.get_eye_y(), position.z);
        let search = player
            .bounding_box()
            .expand_towards(player.look_angle() * 5.0)
            .inflate(1.0);
        if context
            .world
            .get_entities_in_aabb_matching(&search, |entity| {
                entity.id() != player.id() && entity.is_pickable()
            })
            .iter()
            .any(|entity| {
                entity
                    .bounding_box()
                    .inflate(f64::from(entity.get_pick_radius()))
                    .contains(eye)
            })
        {
            return InteractionResult::Pass;
        }
        let stack = context.inv.with_item(|item| item.clone());

        let Some(boat) = self.get_boat(context.world, &hit, &stack, player) else {
            return InteractionResult::Fail;
        };

        boat.set_y_rot(player.rotation().0);
        let collisions = WorldCollisionProvider::for_entity(context.world, boat.as_ref());
        let collision_context =
            BlockCollisionContext::entity(boat.position().y, boat.is_descending())
                .with_fall_distance(boat.fall_distance());
        if collisions.has_collision_with_context(&boat.bounding_box(), collision_context) {
            return InteractionResult::Fail;
        }
        if let Err(error) = add_spawned_entity(context.world, boat) {
            log::warn!("Failed to add boat {}: {error:?}", self.entity_type.key);
            return InteractionResult::Fail;
        }
        context.world.game_event_at(
            &vanilla_game_events::ENTITY_PLACE,
            hit.location,
            &GameEventContext::new(Some(player.as_ref()), None),
        );
        context
            .inv
            .with_item(|item| item.consume_one(player.has_infinite_materials()));
        player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        InteractionResult::Success
    }
}
