
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[cfg(debug_assertions)]
pub const PROXY_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8443);
#[cfg(not(debug_assertions))]
pub const PROXY_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(51, 68, 52, 23)), 443);


#[cfg(debug_assertions)]
pub const PROXY_KEY: (u8, [u8; 32]) = (
    1,
    [
        0x01, 0xd0, 0x17, 0x77, 0x15, 0x42, 0x31, 0x1b, 0x34, 0xb7, 0x81, 0x38, 0x81, 0x0a, 0x14, 0xb5,
        0xc1, 0xe4, 0x4f, 0x69, 0xd2, 0x7b, 0xb2, 0xdf, 0x7e, 0xb0, 0x45, 0x19, 0x04, 0x29, 0x62, 0x3b,
    ],
);

#[cfg(not(debug_assertions))]
pub const PROXY_KEY: (u8, [u8; 32]) = (
    1,
    [
        0x7a, 0xb1, 0xa6, 0xe6, 0xb8, 0xfe, 0x7b, 0x60, 0x31, 0x3e, 0x58, 0xde, 0xf1, 0x39, 0x81, 0xa6,
        0xcc, 0x97, 0x4c, 0x58, 0x68, 0x46, 0xcd, 0xd6, 0x99, 0x08, 0x47, 0x2d, 0x8f, 0x33, 0x6b, 0x05,
    ],
);

#[cfg(not(debug_assertions))]
const _: () = assert!(key_is_set(&PROXY_KEY.1), "ws/config.rs: the proxy's public key is not set");

#[cfg(not(debug_assertions))]
const fn key_is_set(key: &[u8; 32]) -> bool {
    let mut i = 0;
    while i < key.len() {
        if key[i] != 0 {
            return true;
        }
        i += 1;
    }
    false
}


pub const TAG_PROXY: u8 = 0x00;
pub const TAG_RELAY: u8 = 0x01;
pub const TAG_RATES: u8 = 0x02;
pub const TAG_BOOK: u8 = 0x03;

pub const TAG_BTC: u8 = 0x04;
pub const FLAG_ZSTD: u8 = 0x01;
