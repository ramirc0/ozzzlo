//! The Status block: live state of the case and buds.

use super::Function;

pub const GET_ALL: Function = Function { block: 2, id: 1 };
pub const BATTERY_LEVEL: Function = Function { block: 2, id: 2 };
