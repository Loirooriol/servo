/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use bitflags::Flags;
use layout_api::LayoutDamage;
use layout_api::wrapper_traits::{LayoutNode, ThreadSafeLayoutNode};
use script::layout_dom::ServoThreadSafeLayoutNode;
use style::context::{SharedStyleContext, StyleContext};
use style::data::ElementData;
use style::dom::{NodeInfo, TElement, TNode};
use style::selector_parser::RestyleDamage;
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::values::computed::Display;

use crate::context::LayoutContext;
use crate::dom::{DOMLayoutData, NodeExt};

pub struct RecalcStyle<'a> {
    context: &'a LayoutContext<'a>,
}

impl<'a> RecalcStyle<'a> {
    pub(crate) fn new(context: &'a LayoutContext<'a>) -> Self {
        RecalcStyle { context }
    }

    pub(crate) fn context(&self) -> &LayoutContext<'a> {
        self.context
    }
}

#[expect(unsafe_code)]
impl<'dom, E> DomTraversal<E> for RecalcStyle<'_>
where
    E: TElement,
    E::ConcreteNode: 'dom + LayoutNode<'dom>,
{
    fn process_preorder<F>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<E>,
        node: E::ConcreteNode,
        note_child: F,
    ) where
        F: FnMut(E::ConcreteNode),
    {
        if node.is_text_node() {
            return;
        }

        let had_style_data = node.style_data().is_some();
        unsafe {
            node.initialize_style_and_layout_data::<DOMLayoutData>();
        }

        let element = node.as_element().unwrap();
        let mut element_data = element.mutate_data().unwrap();

        if !had_style_data {
            element_data.damage = RestyleDamage::reconstruct();
        }

        recalc_style_at(
            self,
            traversal_data,
            context,
            element,
            &mut element_data,
            note_child,
        );

        unsafe {
            element.unset_dirty_descendants();
        }
    }

    #[inline]
    fn needs_postorder_traversal() -> bool {
        false
    }

    fn process_postorder(&self, _style_context: &mut StyleContext<E>, _node: E::ConcreteNode) {
        panic!("this should never be called")
    }

    fn text_node_needs_traversal(node: E::ConcreteNode, parent_data: &ElementData) -> bool {
        node.layout_data().is_none() || !parent_data.damage.is_empty()
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context.style_context
    }
}

#[servo_tracing::instrument(skip_all)]
pub(crate) fn compute_damage_and_repair_style(
    context: &SharedStyleContext,
    node: ServoThreadSafeLayoutNode<'_>,
    damage_from_parent: RestyleDamage,
) -> RestyleDamage {
    let mut element_damage; // RestyleDamage, element + parent
    let original_element_damage; // LayoutDamage, just element
    let element_data = &node
        .style_data()
        .expect("Should not run `compute_damage` before styling.")
        .element_data;

    {
        let mut element_data = element_data.borrow_mut();
        let damage = std::mem::take(&mut element_data.damage);
        element_damage = damage | damage_from_parent;

        if let Some(ref style) = element_data.styles.primary {
            if style.get_box().display == Display::None {
                return element_damage;
            }
        }

        // Do we need this?
        original_element_damage = LayoutDamage::from_bits_retain(damage.bits());
    }

    // If we are reconstructing this node, then all of the children should be reconstructed as well.
    // Otherwise, do not propagate down its box damage.
    let mut damage_for_children = element_damage;
    if !element_damage.contains(LayoutDamage::rebuild_box_tree()) {
        damage_for_children.truncate();
    }

    for child in node.children() {
        if child.is_element() {
            element_damage |=
                compute_damage_and_repair_style(context, child, damage_for_children);
        }
    }

    // If one of our children needed to be reconstructed, we need to recollect children
    // during box tree construction.
    let recompute_inline_sizes = RestyleDamage::from_bits_retain(LayoutDamage::RECOMPUTE_INLINE_CONTENT_SIZES.bits());
    if element_damage.contains(LayoutDamage::rebuild_box_tree()) {
        node.unset_all_pseudo_boxes();
        return LayoutDamage::recollect_box_tree_children() | recompute_inline_sizes | RestyleDamage::RELAYOUT;
    }
    let mut element_layout_damage = element_damage.into();
    if element_damage.contains(RestyleDamage::RELAYOUT) {
        if let Some(inner_layout_data) = node.inner_layout_data() {
            inner_layout_data.with_each_pseudo_layout_box_base_mut(|base| {
                base.add_damage(element_layout_damage);
            });
            if let Some(self_box) = &*inner_layout_data.self_box.borrow() {
                self_box.with_base_mut(|base| {
                    base.add_damage(element_layout_damage);
                    element_layout_damage |= base.damage;
                });
            }
        }
    }
    if element_layout_damage.has_box_damage() {
        node.unset_all_pseudo_boxes();
    }

    // Only propagate up layout phases from children, as other types of damage are
    // incorporated into `element_damage` above.


    // If the box will be preserved, update the box's style and also in any fragments
    // that haven't been cleared. Meanwhile, clear the damage to avoid affecting the
    // next reflow.
    if !element_layout_damage.has_box_damage() && !original_element_damage.is_empty() {
        node.repair_style(context);
    }
    element_damage & RestyleDamage::RELAYOUT
}
