/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::fmt::{Debug, Formatter};
use std::sync::atomic::AtomicBool;

use app_units::Au;
use atomic_refcell::AtomicRefCell;
use layout_api::LayoutDamage;
use malloc_size_of_derive::MallocSizeOf;
use servo_arc::Arc;
use style::properties::ComputedValues;
use std::sync::atomic::Ordering;

use crate::context::LayoutContext;
use crate::dom::{LayoutBox, WeakLayoutBox};
use crate::formatting_contexts::Baselines;
use crate::fragment_tree::{BaseFragmentInfo, CollapsedBlockMargins, Fragment, SpecificLayoutInfo};
use crate::positioned::PositioningContext;
use crate::sizing::{ComputeInlineContentSizes, InlineContentSizesResult, SizeConstraint};
use crate::{ConstraintSpace, ContainingBlockSize};
use crate::style_ext::ComputedValuesExt;
use style::values::specified::PositionProperty;

/// A box tree node that handles containing information about style and the original DOM
/// node or pseudo-element that it is based on. This also handles caching of layout values
/// such as the inline content sizes to avoid recalculating these values during layout
/// passes.
///
/// In the future, this will hold layout results to support incremental layout.
#[derive(MallocSizeOf)]
pub(crate) struct LayoutBoxBase {
    pub base_fragment_info: BaseFragmentInfo,
    pub style: Arc<ComputedValues>,
    pub cached_inline_content_size:
        AtomicRefCell<Option<Box<(SizeConstraint, InlineContentSizesResult)>>>,
    pub outer_inline_content_sizes_depend_on_content: AtomicBool,
    pub cached_layout_result: AtomicRefCell<Option<Box<CacheableLayoutResultAndInputs>>>,
    pub fragments: AtomicRefCell<Vec<Fragment>>,
    pub parent_box: Option<WeakLayoutBox>,
    pub damage: LayoutDamage,
}

impl LayoutBoxBase {
    pub(crate) fn new(base_fragment_info: BaseFragmentInfo, style: Arc<ComputedValues>) -> Self {
        Self {
            base_fragment_info,
            style,
            cached_inline_content_size: AtomicRefCell::default(),
            outer_inline_content_sizes_depend_on_content: AtomicBool::new(true),
            cached_layout_result: AtomicRefCell::default(),
            fragments: AtomicRefCell::default(),
            parent_box: None,
            damage: LayoutDamage::empty(),
        }
    }

    /// Get the inline content sizes of a box tree node that extends this [`LayoutBoxBase`], fetch
    /// the result from a cache when possible.
    pub(crate) fn inline_content_sizes(
        &self,
        layout_context: &LayoutContext,
        constraint_space: &ConstraintSpace,
        layout_box: &impl ComputeInlineContentSizes,
    ) -> InlineContentSizesResult {
        let mut cache = self.cached_inline_content_size.borrow_mut();
        if let Some(cached_inline_content_size) = cache.as_ref() {
            let (previous_cb_block_size, result) = **cached_inline_content_size;
            if !result.depends_on_block_constraints ||
                previous_cb_block_size == constraint_space.block_size
            {
                return result;
            }
            // TODO: Should we keep multiple caches for various block sizes?
        }

        let result =
            layout_box.compute_inline_content_sizes_with_fixup(layout_context, constraint_space);
        *cache = Some(Box::new((constraint_space.block_size, result)));
        result
    }

    pub(crate) fn fragments(&self) -> Vec<Fragment> {
        self.fragments.borrow().clone()
    }

    pub(crate) fn add_fragment(&self, fragment: Fragment) {
        self.fragments.borrow_mut().push(fragment);
    }

    pub(crate) fn set_fragment(&self, fragment: Fragment) {
        *self.fragments.borrow_mut() = vec![fragment];
    }

    pub(crate) fn clear_fragments(&self) {
        self.fragments.borrow_mut().clear();
    }

    pub(crate) fn repair_style(&mut self, new_style: &Arc<ComputedValues>) {
        self.style = new_style.clone();
        for fragment in self.fragments.borrow_mut().iter_mut() {
            if let Some(mut base) = fragment.base_mut() {
                base.repair_style(new_style);
            }
        }
    }

    pub(crate) fn parent_box(&self) -> Option<LayoutBox> {
        self.parent_box.as_ref().and_then(WeakLayoutBox::upgrade)
    }

    /// For absolutely positioned boxes, this returns the containing block.
    /// In other cases, it returns the parent box.
    #[expect(unused)]
    fn container(&self) -> Option<LayoutBox> {
        let filter = match self.style.get_box().position {
            PositionProperty::Absolute => {
                ComputedValuesExt::establishes_containing_block_for_absolute_descendants
            },
            PositionProperty::Fixed => {
                ComputedValuesExt::establishes_containing_block_for_all_descendants
            },
            _ => return self.parent_box(),
        };
        let mut ancestor = self.parent_box();
        while let Some(ref ancestor_ref) = ancestor {
            let Some(next_ancestor) = ancestor_ref.with_base(|base| {
                if filter(&*base.style, base.base_fragment_info.flags) {
                    None
                } else {
                    Some(base.parent_box())
                }
            })?
            else {
                return ancestor;
            };
            ancestor = next_ancestor;
        }
        None
    }

    pub(crate) fn add_damage(&mut self, damage: LayoutDamage) {
        if self.damage.contains(damage) {
            return;
        }
        self.damage |= damage;
        self.clear_fragments();
        *self.cached_layout_result.borrow_mut() = None;
        if damage.contains(LayoutDamage::RECOMPUTE_INLINE_CONTENT_SIZES) {
            *self.cached_inline_content_size.borrow_mut() = None;
        }

        let mut damage_for_parent = self.damage;

        // When a block container has a mix of inline-level and block-level contents,
        // the inline-level ones are wrapped inside an anonymous block associated with
        // the block container. The anonymous block has an `auto` size, so its intrinsic
        // contribution depends on content, but it can't affect the intrinsic size of
        // ancestors if the block container is sized extrinsically.
        if self.base_fragment_info.is_anonymous() || !self.outer_inline_content_sizes_depend_on_content.load(Ordering::Relaxed) {
            // If the intrinsic contributions of this node depend on content, we will need to clear
            // the cached intrinsic sizes of the parent. But if the contributions are purely extrinsic,
            // then the intrinsic sizes of the ancestors won't be affected, and we can keep the cache.
            damage_for_parent.remove(LayoutDamage::RECOMPUTE_INLINE_CONTENT_SIZES)
        }
        // If we have to rebuild the box, the damage propagation is taken care of in the traversal.
        if !self.damage.contains(LayoutDamage::REBUILD_BOX) {
            if let Some(container) = self.container() {
                container.with_base_mut(|base| {
                    base.add_damage(damage_for_parent)
                });
            }
        }
    }
}

impl Debug for LayoutBoxBase {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("LayoutBoxBase").finish()
    }
}

#[derive(Clone, MallocSizeOf)]
pub(crate) struct CacheableLayoutResult {
    pub fragments: Vec<Fragment>,

    /// <https://drafts.csswg.org/css2/visudet.html#root-height>
    pub content_block_size: Au,

    /// If this layout is for a block container, this tracks the collapsable size
    /// of start and end margins and whether or not the block container collapsed through.
    pub collapsible_margins_in_children: CollapsedBlockMargins,

    /// The contents of a table may force it to become wider than what we would expect
    /// from 'width' and 'min-width'. This is the resulting inline content size,
    /// or None for non-table layouts.
    pub content_inline_size_for_table: Option<Au>,

    /// The offset of the last inflow baseline of this layout in the content area, if
    /// there was one. This is used to propagate baselines to the ancestors of `display:
    /// inline-block`.
    pub baselines: Baselines,

    /// Whether or not this layout depends on the containing block size.
    pub depends_on_block_constraints: bool,

    /// Additional information of this layout that could be used by Javascripts and devtools.
    pub specific_layout_info: Option<SpecificLayoutInfo>,
}

/// A collection of layout inputs and a cached layout result for a [`LayoutBoxBase`].
#[derive(MallocSizeOf)]
pub(crate) struct CacheableLayoutResultAndInputs {
    /// The [`CacheableLayoutResult`] for this layout.
    pub result: CacheableLayoutResult,

    /// The [`ContainingBlockSize`] to use for this box's contents, but not
    /// for the box itself.
    pub containing_block_for_children_size: ContainingBlockSize,

    /// A [`PositioningContext`] holding absolutely-positioned descendants
    /// collected during the layout of this box.
    pub positioning_context: PositioningContext,
}
