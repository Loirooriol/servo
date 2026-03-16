/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::str::DOMString;
use crate::dom::htmlinputelement::SpecificInputType;
use crate::dom::types::HTMLInputElement;

pub(crate) struct NumberInputType ();

impl SpecificInputType for NumberInputType {
    fn sanitize_value(&self, _input: &HTMLInputElement, value: &mut DOMString) {
        if !value.is_valid_floating_point_number_string() {
            value.clear();
        }
        // Spec says that user agent "may" round the value
        // when it's suffering a step mismatch, but WPT tests
        // want it unrounded, and this matches other browser
        // behavior (typing an unrounded number into an
        // integer field box and pressing enter generally keeps
        // the number intact but makes the input box :invalid)
    }
}
