/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::str::DOMString;
use crate::dom::htmlinputelement::SpecificInputType;
use crate::dom::types::HTMLInputElement;

pub(crate) struct ColorInputType ();

impl SpecificInputType for ColorInputType {
    fn sanitize_value(&self, input: &HTMLInputElement, value: &mut DOMString) {
        // > The value sanitization algorithm is as follows:
        // > Run update a color well control color for the element.
        input.update_a_color_well_control_color(value);
    }
}
