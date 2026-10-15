
pub const BASE_RESERVE: f64 = 1.0;

pub fn is_activated(total_xrp: f64) -> bool {
    total_xrp >= BASE_RESERVE
}
