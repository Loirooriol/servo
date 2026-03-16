/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::str::DOMString;
use crate::dom::htmlinputelement::SpecificInputType;
use crate::dom::types::HTMLInputElement;

pub(crate) struct UrlInputType ();

impl SpecificInputType for UrlInputType {
    fn sanitize_value(&self, _input: &HTMLInputElement, value: &mut DOMString) {
        value.strip_newlines();
        value.strip_leading_and_trailing_ascii_whitespace();
    }
}
