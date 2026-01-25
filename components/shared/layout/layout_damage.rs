/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use bitflags::bitflags;
use style::selector_parser::RestyleDamage;

use crate::malloc_size_of_is_0;

bitflags! {
    /// Individual layout actions that may be necessary after restyling. This is an extension
    /// of `RestyleDamage` from stylo, which only uses the 4 lower bits.
    #[derive(Clone, Copy, Default, Eq, PartialEq)]
    pub struct LayoutDamage: u16 {
        const REBUILD_FRAGMENT = 0b0000_0000_0001 << 4;
        /// Recollect the box children for this element, because some of the them will be
        /// rebuilt.
        const RECOLLECT_BOX_TREE_CHILDREN = 0b0000_0000_0010 << 4;
        /// Clear the cached inline content sizes and recompute them during the next layout.
        const RECOMPUTE_INLINE_CONTENT_SIZES = 0b0000_0000_0100 << 4;
        /// Rebuild the entire box for this element, which means that every part of layout
        /// needs to happen again.
        const REBUILD_BOX = 0b1111_1111_1111 << 4;
    }
}

malloc_size_of_is_0!(LayoutDamage);

impl LayoutDamage {
    pub fn recollect_box_tree_children() -> RestyleDamage {
        RestyleDamage::from_bits_retain(LayoutDamage::RECOLLECT_BOX_TREE_CHILDREN.bits())
    }

    pub fn rebuild_box_tree() -> RestyleDamage {
        RestyleDamage::from_bits_retain(LayoutDamage::REBUILD_BOX.bits())
    }

    pub fn has_box_damage(&self) -> bool {
        self.contains(Self::RECOLLECT_BOX_TREE_CHILDREN)
    }
}

impl From<RestyleDamage> for LayoutDamage {
    fn from(restyle_damage: RestyleDamage) -> Self {
        LayoutDamage::from_bits_retain(restyle_damage.bits())
    }
}

impl std::fmt::Debug for LayoutDamage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.contains(Self::REBUILD_BOX) {
            f.write_str("REBUILD_BOX")
        } else if self.contains(Self::RECOLLECT_BOX_TREE_CHILDREN) {
            f.write_str("RECOLLECT_BOX_TREE_CHILDREN")
        } else {
            f.write_str("EMPTY")
        }
    }
}
