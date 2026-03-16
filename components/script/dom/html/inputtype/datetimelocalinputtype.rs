/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::dom::bindings::str::DOMString;
use crate::dom::htmlinputelement::SpecificInputType;
use crate::dom::types::HTMLInputElement;
use crate::dom::bindings::str::FromInputValueString;
use crate::dom::bindings::str::ToInputValueString;

pub(crate) struct DatetimeLocalInputType ();

impl SpecificInputType for DatetimeLocalInputType {
    fn sanitize_value(&self, _input: &HTMLInputElement, value: &mut DOMString) {
        let time = value
            .str()
            .parse_local_date_time_string()
            .map(|date_time| date_time.to_local_date_time_string());
        match time {
            Some(normalized_string) => *value = normalized_string.into(),
            None => value.clear(),
        }
    }
}
