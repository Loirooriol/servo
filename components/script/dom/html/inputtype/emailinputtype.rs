/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::str::DOMString;
use crate::dom::htmlinputelement::SpecificInputType;
use crate::dom::types::HTMLInputElement;
use style::str::split_commas;
use script_bindings::codegen::GenericBindings::HTMLInputElementBinding::HTMLInputElementMethods;
use itertools::Itertools;

pub(crate) struct EmailInputType ();

impl SpecificInputType for EmailInputType {
    fn sanitize_value(&self, input: &HTMLInputElement, value: &mut DOMString) {
        if !input.Multiple() {
            value.strip_newlines();
            value.strip_leading_and_trailing_ascii_whitespace();
        } else {
            let sanitized = split_commas(&value.str())
                .map(|token| {
                    let mut token = DOMString::from(token.to_string());
                    token.strip_newlines();
                    token.strip_leading_and_trailing_ascii_whitespace();
                    token
                })
                .join(",");
            value.clear();
            value.push_str(sanitized.as_str());
        }
    }
}
