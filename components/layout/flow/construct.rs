/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use layout_api::LayoutNode;
use servo_arc::Arc;
use style::properties::ComputedValues;
use style::properties::longhands::list_style_position::computed_value::T as ListStylePosition;
use style::str::char_is_whitespace;

use super::OutsideMarker;
use super::inline::construct::InlineFormattingContextBuilder;
use super::inline::inline_box::InlineBox;
use super::inline::{InlineFormattingContext, SharedInlineStyles};
use crate::PropagatedBoxTreeData;
use crate::cell::ArcRefCell;
use crate::context::LayoutContext;
use crate::dom::{BoxSlot, LayoutBox, NodeExt};
use crate::dom_traversal::{
    BoxTreeString, Contents, NodeAndStyleInfo, NonReplacedContents, PseudoElementContentItem,
    TraversalHandler,
};
use crate::flow::float::FloatBox;
use crate::flow::same_formatting_context_block::SameFormattingContextBlock;
use crate::flow::{BlockContainer, BlockFormattingContext, BlockLevelBox};
use crate::formatting_contexts::{
    IndependentFormattingContext, IndependentFormattingContextContents,
};
use crate::fragment_tree::FragmentFlags;
use crate::layout_box_base::LayoutBoxBase;
use crate::positioned::AbsolutelyPositionedBox;
use crate::style_ext::{ComputedValuesExt, DisplayGeneratingBox, DisplayInside, DisplayOutside};
use crate::table::{AnonymousTableContent, AnonymousTableContentElement, Table};

impl BlockFormattingContext {
    pub(crate) fn construct(
        context: &LayoutContext,
        info: &NodeAndStyleInfo<'_>,
        contents: NonReplacedContents,
        propagated_data: PropagatedBoxTreeData,
        is_list_item: bool,
    ) -> Self {
        Self::from_block_container(BlockContainer::construct(
            context,
            info,
            contents,
            propagated_data,
            is_list_item,
        ))
    }

    pub(crate) fn from_block_container(contents: BlockContainer) -> Self {
        let contains_floats = contents.contains_floats();
        Self {
            contents,
            contains_floats,
        }
    }
}

struct BlockLevelJob<'dom> {
    info: NodeAndStyleInfo<'dom>,
    box_slot: BoxSlot<'dom>,
    propagated_data: PropagatedBoxTreeData,
    kind: BlockLevelCreator,
}

pub(crate) enum BlockLevelCreator {
    SameFormattingContextBlock {
        contents: NonReplacedContents,
        propagated_data: PropagatedBoxTreeData,
        is_list_item: bool,
    },
    Independent {
        display_inside: DisplayInside,
        contents: Contents,
    },
    OutsideMarker {
        list_item_style: Arc<ComputedValues>,
        contents: Vec<PseudoElementContentItem>,
    },
    AnonymousTable {
        table_block: ArcRefCell<BlockLevelBox>,
    },
}

impl BlockLevelCreator {
    pub(crate) fn new_for_inflow_block_level_element<'dom>(
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
        propagated_data: PropagatedBoxTreeData,
    ) -> Self {
        match contents {
            Contents::NonReplaced(contents) => match display_inside {
                DisplayInside::Flow { is_list_item }
                    // Fragment flags are just used to indicate whether the element is replaced or a widget,
                    // and whether it's a body or root propagating its `overflow` to the viewport. We have
                    // already checked that the former is not the case.
                    // TODO(#39932): empty flags are wrong when propagating `overflow` to the viewport.
                    if !info.style.establishes_block_formatting_context(
                        FragmentFlags::empty()
                    ) =>
                {
                    Self::SameFormattingContextBlock {
                        contents,
                        propagated_data,
                        is_list_item,
                    }
                },
                _ => Self::Independent {
                    display_inside,
                    contents: Contents::NonReplaced(contents),
                },
            },
            Contents::Replaced(_) | Contents::Widget(_) => Self::Independent {
                display_inside,
                contents,
            },
        }
    }
}

/// A builder for a block container.
///
/// This builder starts from the first child of a given DOM node
/// and does a preorder traversal of all of its inclusive siblings.
pub(crate) struct BlockContainerBuilder<'dom, 'style> {
    context: &'style LayoutContext<'style>,

    /// This NodeAndStyleInfo contains the root node, the corresponding pseudo
    /// content designator, and the block container style.
    info: &'style NodeAndStyleInfo<'dom>,

    /// Whether or not this builder has yet produced an inline formatting context
    /// with the first formatted line:
    /// <https://www.w3.org/TR/css-pseudo-4/#first-formatted-line>.
    /// This is used for `text-indent` and `::first-letter`.
    have_already_seen_first_formatted_line: bool,

    /// The propagated data to use for BoxTree construction.
    propagated_data: PropagatedBoxTreeData,

    /// The [`InlineFormattingContextBuilder`] if we have encountered any inline items,
    /// otherwise None.
    ///
    /// TODO: This can be `OnceCell` once `OnceCell::get_mut_or_init` is stabilized.
    inline_formatting_context_builder: InlineFormattingContextBuilder,

    /// A collection of content that is being added to an anonymous table. This is
    /// composed of any sequence of internal table elements or table captions that
    /// are found outside of a table.
    anonymous_table_content: Vec<AnonymousTableContent<'dom>>,

    /// Any [`InlineFormattingContexts`] created need to know about the ongoing `display: contents`
    /// ancestors that have been processed. This `Vec` allows passing those into new
    /// [`InlineFormattingContext`]s that we create.
    display_contents_shared_styles: Vec<SharedInlineStyles>,
}

impl BlockContainer {
    pub fn construct(
        context: &LayoutContext,
        info: &NodeAndStyleInfo<'_>,
        contents: NonReplacedContents,
        propagated_data: PropagatedBoxTreeData,
        is_list_item: bool,
    ) -> BlockContainer {
        let mut builder = BlockContainerBuilder::new(context, info, propagated_data);

        if is_list_item &&
            let Some((marker_info, marker_contents)) = crate::lists::make_marker(context, info)
        {
            match marker_info.style.get_list_style_position() {
                ListStylePosition::Inside => {
                    builder.handle_list_item_marker_inside(&marker_info, marker_contents)
                },
                ListStylePosition::Outside => builder.handle_list_item_marker_outside(
                    &marker_info,
                    marker_contents,
                    info.style.clone(),
                ),
            }
        }

        contents.traverse(context, info, &mut builder);
        builder.finish()
    }
}

impl<'dom, 'style> BlockContainerBuilder<'dom, 'style> {
    pub(crate) fn new(
        context: &'style LayoutContext,
        info: &'style NodeAndStyleInfo<'dom>,
        propagated_data: PropagatedBoxTreeData,
    ) -> Self {
        BlockContainerBuilder {
            context,
            info,
            propagated_data,
            have_already_seen_first_formatted_line: false,
            anonymous_table_content: Vec::new(),
            inline_formatting_context_builder: InlineFormattingContextBuilder::new(
                info, context, true, /* should_process_first_letter */
            ),
            display_contents_shared_styles: Vec::new(),
        }
    }

    fn currently_processing_inline_box(&self) -> bool {
        self.inline_formatting_context_builder
            .currently_processing_inline_box()
    }

    fn finish_ongoing_inline_formatting_context(&mut self) -> InlineFormattingContext {
        // ::first-letter must be on the first formatted line.
        let should_process_first_letter = !self.have_already_seen_first_formatted_line;
        let mut builder = InlineFormattingContextBuilder::new(
            self.info,
            self.context,
            should_process_first_letter,
        );
        for shared_inline_styles in self.display_contents_shared_styles.iter() {
            builder.enter_display_contents(shared_inline_styles.clone());
        }
        std::mem::swap(&mut self.inline_formatting_context_builder, &mut builder);
        builder.finish(
            self.context,
            !self.have_already_seen_first_formatted_line,
            self.info.node.is_single_line_text_input(),
            self.info.style.to_bidi_level(),
        )
    }

    pub(crate) fn finish(mut self) -> BlockContainer {
        debug_assert!(!self.currently_processing_inline_box());

        self.finish_anonymous_table_if_needed();

        BlockContainer::InlineFormattingContext(self.finish_ongoing_inline_formatting_context())
    }

    fn finish_anonymous_table_if_needed(&mut self) {
        if self.anonymous_table_content.is_empty() {
            return;
        }

        // From https://drafts.csswg.org/css-tables/#fixup-algorithm:
        //  > If the box’s parent is an inline, run-in, or ruby box (or any box that would perform
        //  > inlinification of its children), then an inline-table box must be generated; otherwise
        //  > it must be a table box.
        //
        // Note that text content in the inline formatting context isn't enough to force the
        // creation of an inline table. It requires the parent to be an inline box.
        let inline_table = self.currently_processing_inline_box();

        let mut contents: Vec<AnonymousTableContent<'dom>> =
            std::mem::take(&mut self.anonymous_table_content);
        let last_element_index = contents
            .iter()
            .rposition(|content| matches!(content, AnonymousTableContent::Element { .. }))
            .expect("Anonymous table contents should include some table-level element");
        let trailing_contents = contents.split_off(last_element_index + 1);

        let (table_info, ifc) = Table::construct_anonymous(
            self.context,
            self,
            self.info,
            contents,
            self.propagated_data,
        );

        if inline_table {
            self.inline_formatting_context_builder
                .push_atomic(|| ArcRefCell::new(ifc), None);
        } else {
            let table_block = ArcRefCell::new(BlockLevelBox::Independent(ifc));
            let box_slot = table_info.node.box_slot();
            let job = BlockLevelJob {
                info: table_info,
                box_slot,
                kind: BlockLevelCreator::AnonymousTable { table_block },
                propagated_data: self.propagated_data,
            };
            self.inline_formatting_context_builder
                .push_block_level_box(job.finish(self.context));
        }

        // If the anonymous table contents end with trailing whitespace, that
        // whitespace doesn't actually belong to the table. It should be processed outside
        // ie become a space between the anonymous table and the rest of the block
        // content. Anonymous tables are really only constructed around internal table
        // elements and the whitespace between them, so this trailing whitespace should
        // not be included.
        //
        // See https://drafts.csswg.org/css-tables/#fixup-algorithm sections "Remove
        // irrelevant boxes" and "Generate missing parents."
        for content in trailing_contents {
            match content {
                AnonymousTableContent::Text(info, text) => self.handle_text(&info, text),
                AnonymousTableContent::EnterDisplayContents(styles) => {
                    self.enter_display_contents(styles)
                },
                AnonymousTableContent::LeaveDisplayContents => self.leave_display_contents(),
                AnonymousTableContent::Element { .. } => {
                    unreachable!("All elements were placed inside the table")
                },
            }
        }
    }
}

impl<'dom> TraversalHandler<'dom> for BlockContainerBuilder<'dom, '_> {
    fn handle_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display: DisplayGeneratingBox,
        contents: Contents,
        box_slot: BoxSlot<'dom>,
    ) {
        match display {
            DisplayGeneratingBox::OutsideInside { outside, inside } => {
                self.finish_anonymous_table_if_needed();

                match outside {
                    DisplayOutside::Inline => {
                        self.handle_inline_level_element(info, inside, contents, box_slot)
                    },
                    DisplayOutside::Block => {
                        let box_style = info.style.get_box();
                        // Floats and abspos cause blockification, so they only happen in this case.
                        // https://drafts.csswg.org/css2/visuren.html#dis-pos-flo
                        if box_style.position.is_absolutely_positioned() {
                            self.handle_absolutely_positioned_element(
                                info, inside, contents, box_slot,
                            )
                        } else if box_style.float.is_floating() {
                            self.handle_float_element(info, inside, contents, box_slot)
                        } else {
                            self.handle_block_level_element(info, inside, contents, box_slot)
                        }
                    },
                };
            },
            DisplayGeneratingBox::LayoutInternal(_) => {
                self.anonymous_table_content
                    .push(AnonymousTableContent::Element(Box::new(
                        AnonymousTableContentElement {
                            info: info.clone(),
                            display,
                            contents,
                            box_slot,
                        },
                    )));
            },
        }
    }

    fn handle_text(&mut self, info: &NodeAndStyleInfo<'dom>, text: BoxTreeString<'dom>) {
        if text.is_empty() && !info.node.text_node_needs_caret_placeholder() {
            return;
        }

        // If we are building an anonymous table ie this text directly followed internal
        // table elements that did not have a `<table>` ancestor, then we forward all
        // whitespace to the table builder.
        if !self.anonymous_table_content.is_empty() && text.chars().all(char_is_whitespace) {
            self.anonymous_table_content
                .push(AnonymousTableContent::Text(info.clone(), text));
            return;
        } else {
            self.finish_anonymous_table_if_needed();
        }

        self.inline_formatting_context_builder
            .push_text_with_possible_first_letter(text, info, self.info, self.context);
    }

    fn enter_display_contents(&mut self, styles: SharedInlineStyles) {
        if !self.anonymous_table_content.is_empty() {
            self.anonymous_table_content
                .push(AnonymousTableContent::EnterDisplayContents(styles));
            return;
        }
        self.display_contents_shared_styles.push(styles.clone());
        self.inline_formatting_context_builder
            .enter_display_contents(styles);
    }

    fn leave_display_contents(&mut self) {
        if !self.anonymous_table_content.is_empty() {
            self.anonymous_table_content
                .push(AnonymousTableContent::LeaveDisplayContents);
            return;
        }
        self.display_contents_shared_styles.pop();
        self.inline_formatting_context_builder
            .leave_display_contents();
    }
}

impl<'dom> BlockContainerBuilder<'dom, '_> {
    fn handle_list_item_marker_inside(
        &mut self,
        marker_info: &NodeAndStyleInfo<'dom>,
        contents: Vec<crate::dom_traversal::PseudoElementContentItem>,
    ) {
        let box_slot = marker_info.node.box_slot();
        self.handle_inline_level_element(
            marker_info,
            DisplayInside::Flow {
                is_list_item: false,
            },
            Contents::for_pseudo_element(contents),
            box_slot,
        );
    }

    fn handle_list_item_marker_outside(
        &mut self,
        marker_info: &NodeAndStyleInfo<'dom>,
        contents: Vec<crate::dom_traversal::PseudoElementContentItem>,
        list_item_style: Arc<ComputedValues>,
    ) {
        let box_slot = marker_info.node.box_slot();
        let job = BlockLevelJob {
            info: marker_info.clone(),
            box_slot,
            kind: BlockLevelCreator::OutsideMarker {
                contents,
                list_item_style,
            },
            propagated_data: self.propagated_data,
        };
        self.inline_formatting_context_builder
            .push_block_level_box(job.finish(self.context));
    }

    fn handle_inline_level_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
        box_slot: BoxSlot<'dom>,
    ) {
        let context = self.context;
        let old_layout_box = box_slot.take_layout_box();
        let (is_list_item, non_replaced_contents) = match (display_inside, contents) {
            (
                DisplayInside::Flow { is_list_item },
                Contents::NonReplaced(non_replaced_contents),
            ) => (is_list_item, non_replaced_contents),
            (_, contents) => {
                // If this inline element is an atomic, handle it and return.
                let propagated_data = self.propagated_data;

                let construction_callback = || {
                    ArcRefCell::new(IndependentFormattingContext::construct(
                        context,
                        info,
                        display_inside,
                        contents,
                        propagated_data,
                    ))
                };

                let atomic = self
                    .inline_formatting_context_builder
                    .push_atomic(construction_callback, old_layout_box);
                box_slot.set(LayoutBox::InlineLevel(atomic));
                return;
            },
        };

        // Otherwise, this is just a normal inline box. Whatever happened before, all we need to do
        // before recurring is to remember this ongoing inline level box.
        let inline_builder = &mut self.inline_formatting_context_builder;
        let inline_item = inline_builder.start_inline_box(
            || ArcRefCell::new(InlineBox::new(info, context)),
            old_layout_box,
        );
        box_slot.set(LayoutBox::InlineLevel(inline_item));

        if info.node.is_html_wbr_element() {
            inline_builder.push_soft_wrap_opportunity();
        }

        if is_list_item &&
            let Some((marker_info, marker_contents)) =
                crate::lists::make_marker(self.context, info)
        {
            // Ignore `list-style-position` here:
            // “If the list item is an inline box: this value is equivalent to `inside`.”
            // https://drafts.csswg.org/css-lists/#list-style-position-outside
            self.handle_list_item_marker_inside(&marker_info, marker_contents)
        }

        non_replaced_contents.traverse(self.context, info, self);

        self.finish_anonymous_table_if_needed();

        self.inline_formatting_context_builder.end_inline_box();
    }

    fn handle_block_level_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
        box_slot: BoxSlot<'dom>,
    ) {
        let propagated_data = self.propagated_data;
        let kind = BlockLevelCreator::new_for_inflow_block_level_element(
            info,
            display_inside,
            contents,
            propagated_data,
        );
        let job = BlockLevelJob {
            info: info.clone(),
            box_slot,
            kind,
            propagated_data,
        };
        self.inline_formatting_context_builder
            .push_block_level_box(job.finish(self.context));
    }

    fn handle_absolutely_positioned_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
        box_slot: BoxSlot<'dom>,
    ) {
        let constructor = || {
            ArcRefCell::new(AbsolutelyPositionedBox::construct(
                self.context,
                info,
                display_inside,
                contents,
            ))
        };
        let old_layout_box = box_slot.take_layout_box();
        let inline_level_box = self
            .inline_formatting_context_builder
            .push_absolutely_positioned_box(constructor, old_layout_box);
        box_slot.set(LayoutBox::InlineLevel(inline_level_box));
    }

    fn handle_float_element(
        &mut self,
        info: &NodeAndStyleInfo<'dom>,
        display_inside: DisplayInside,
        contents: Contents,
        box_slot: BoxSlot<'dom>,
    ) {
        let constructor = || {
            ArcRefCell::new(FloatBox::construct(
                self.context,
                info,
                display_inside,
                contents,
                self.propagated_data,
            ))
        };
        let old_layout_box = box_slot.take_layout_box();
        let inline_level_box = self
            .inline_formatting_context_builder
            .push_float_box(constructor, old_layout_box);
        box_slot.set(LayoutBox::InlineLevel(inline_level_box));
    }
}

impl BlockLevelJob<'_> {
    fn finish(self, context: &LayoutContext) -> ArcRefCell<BlockLevelBox> {
        let info = &self.info;

        // If this `BlockLevelBox` exists, it has been laid out before and is
        // reusable.
        if let Some(block_level_box) = match &*self.box_slot.slot.borrow() {
            Some(LayoutBox::BlockLevel(block_level_box)) => Some(block_level_box.clone()),
            _ => None,
        } {
            return block_level_box;
        }

        let block_level_box = match self.kind {
            BlockLevelCreator::SameFormattingContextBlock {
                contents,
                propagated_data,
                is_list_item,
            } => {
                let contents = BlockContainer::construct(
                    context,
                    info,
                    contents,
                    propagated_data,
                    is_list_item,
                );
                let contains_floats = contents.contains_floats();

                let base = LayoutBoxBase::new(info.into(), info.style.clone());
                base.set_subtree_size(contents.subtree_size() + 1);

                ArcRefCell::new(BlockLevelBox::SameFormattingContextBlock(
                    SameFormattingContextBlock::new(base, contents, contains_floats),
                ))
            },
            BlockLevelCreator::Independent {
                display_inside,
                contents,
            } => {
                let context = IndependentFormattingContext::construct(
                    context,
                    info,
                    display_inside,
                    contents,
                    self.propagated_data,
                );
                ArcRefCell::new(BlockLevelBox::Independent(context))
            },
            BlockLevelCreator::OutsideMarker {
                contents,
                list_item_style,
            } => {
                let contents = NonReplacedContents::OfPseudoElement(contents);
                let block_container = BlockContainer::construct(
                    context,
                    info,
                    contents,
                    self.propagated_data,
                    false, /* is_list_item */
                );
                // An outside ::marker must establish a BFC, and can't contain floats.
                let block_formatting_context = BlockFormattingContext {
                    contents: block_container,
                    contains_floats: false,
                };
                ArcRefCell::new(BlockLevelBox::OutsideMarker(OutsideMarker {
                    context: IndependentFormattingContext::new(
                        LayoutBoxBase::new(info.into(), info.style.clone()),
                        IndependentFormattingContextContents::Flow(block_formatting_context),
                        self.propagated_data,
                    ),
                    list_item_style,
                }))
            },
            BlockLevelCreator::AnonymousTable { table_block } => table_block,
        };
        self.box_slot
            .set(LayoutBox::BlockLevel(block_level_box.clone()));
        block_level_box
    }
}
