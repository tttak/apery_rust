#![cfg_attr(
    feature = "cargo-clippy",
    allow(clippy::cognitive_complexity, clippy::too_many_arguments)
)]
#[macro_use]
extern crate custom_derive;
#[macro_use]
extern crate derive_more;
#[macro_use]
extern crate enum_derive;
#[macro_use]
extern crate lazy_static;
mod authors;
mod bitboard;
mod engine_name;
mod evaluate;
mod file_to_vec;
mod hand;
mod movegen;
mod movepick;
mod piecevalue;
mod position;
mod search;
mod sfen;
mod thread;
mod timeman;
mod tt;
mod types;
pub mod usi;
mod usioption;
