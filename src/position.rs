use crate::bitboard::*;
use crate::evaluate::*;
use crate::hand::*;
use crate::movegen::*;
use crate::piecevalue::*;
use crate::sfen::*;
use crate::types::*;
use rand::prelude::*;
use rand::{Rng, SeedableRng};
use std::convert::TryFrom;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

pub trait IsSearchingTrait {
    const IS_SEARCHING: bool;
}

pub struct SearchingType;
pub struct NotSearchingType;

impl IsSearchingTrait for SearchingType {
    const IS_SEARCHING: bool = true;
}
impl IsSearchingTrait for NotSearchingType {
    const IS_SEARCHING: bool = false;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Repetition {
    Not,
    Draw,
    Win,
    Lose,
    Superior,
    Inferior,
}

#[derive(Clone)]
pub struct CheckInfo {
    blockers_and_pinners_for_king: [(Bitboard, Bitboard); Color::NUM], // color is color_of_king
    check_squares: [Bitboard; PieceType::NUM],
}

impl CheckInfo {
    pub const ZERO: CheckInfo = CheckInfo {
        blockers_and_pinners_for_king: [(Bitboard::ZERO, Bitboard::ZERO); Color::NUM],
        check_squares: [Bitboard::ZERO; PieceType::NUM],
    };
    fn new(pos: &PositionBase) -> CheckInfo {
        let us = pos.side_to_move();
        let them = us.inverse();
        let ksq = pos.king_square(them);
        let bishop_check_squares = ATTACK_TABLE.bishop.magic(ksq).attack(&pos.occupied_bb());
        let rook_check_squares = ATTACK_TABLE.rook.magic(ksq).attack(&pos.occupied_bb());
        let gold_check_squares = ATTACK_TABLE.gold.attack(them, ksq);
        CheckInfo {
            blockers_and_pinners_for_king: [
                pos.slider_blockers_and_pinners(
                    &pos.pieces_c(Color::WHITE),
                    Color::WHITE,
                    pos.king_square(Color::BLACK),
                ),
                pos.slider_blockers_and_pinners(
                    &pos.pieces_c(Color::BLACK),
                    Color::BLACK,
                    pos.king_square(Color::WHITE),
                ),
            ],
            check_squares: [
                Bitboard::ZERO,                                           // PieceType::OCCUPIED
                ATTACK_TABLE.pawn.attack(them, ksq),                      // PieceType::PAWN
                ATTACK_TABLE.lance.attack(them, ksq, &pos.occupied_bb()), // PieceType::LANCE
                ATTACK_TABLE.knight.attack(them, ksq),                    // PieceType::KNIGHT
                ATTACK_TABLE.silver.attack(them, ksq),                    // PieceType::SILVER
                bishop_check_squares,                                     // PieceType::BISHOP
                rook_check_squares,                                       // PieceType::ROOK
                gold_check_squares,                                       // PieceType::GOLD
                Bitboard::ZERO,                                           // PieceType::KING
                gold_check_squares,                                       // PieceType::PRO_PAWN
                gold_check_squares,                                       // PieceType::PRO_LANCE
                gold_check_squares,                                       // PieceType::PRO_KNIGHT
                gold_check_squares,                                       // PieceType::PRO_SILVER
                bishop_check_squares | ATTACK_TABLE.king.attack(ksq),     // PieceType::HORSE
                rook_check_squares | ATTACK_TABLE.king.attack(ksq),       // PieceType::DRAGON
            ],
        }
    }
    fn blockers_for_king(&self, color_of_king: Color) -> Bitboard {
        debug_assert!((color_of_king.0 as usize) < Color::NUM);
        unsafe {
            self.blockers_and_pinners_for_king
                .get_unchecked(color_of_king.0 as usize)
                .0
        }
    }
    fn pinners_for_king(&self, color_of_king: Color) -> Bitboard {
        debug_assert!((color_of_king.0 as usize) < Color::NUM);
        unsafe {
            self.blockers_and_pinners_for_king
                .get_unchecked(color_of_king.0 as usize)
                .1
        }
    }
}

struct Zobrist {
    field: [[[Key; Color::NUM]; Square::NUM]; PieceType::NUM],
    hand: [[[Key; Color::NUM]; 19]; PieceType::NUM], // 19 is max_hand_pawn + 1.
}

impl Zobrist {
    pub const COLOR: Key = Key(1);
    fn get_field(pt: PieceType, sq: Square, c: Color) -> Key {
        debug_assert!(0 <= pt.0 && (pt.0 as usize) < ZOBRIST_TABLES.field.len());
        debug_assert!(0 <= sq.0 && (sq.0 as usize) < ZOBRIST_TABLES.field[pt.0 as usize].len());
        debug_assert!(
            0 <= c.0 && (c.0 as usize) < ZOBRIST_TABLES.field[pt.0 as usize][sq.0 as usize].len()
        );
        unsafe {
            *ZOBRIST_TABLES
                .field
                .get_unchecked(pt.0 as usize)
                .get_unchecked(sq.0 as usize)
                .get_unchecked(c.0 as usize)
        }
    }
    fn get_hand(pt: PieceType, i: u32, c: Color) -> Key {
        debug_assert!(0 <= pt.0 && (pt.0 as usize) < ZOBRIST_TABLES.hand.len());
        debug_assert!(/*0 <= i &&*/ (i as usize) < ZOBRIST_TABLES.hand[pt.0 as usize].len());
        debug_assert!(
            0 <= c.0 && (c.0 as usize) < ZOBRIST_TABLES.hand[pt.0 as usize][i as usize].len()
        );
        unsafe {
            *ZOBRIST_TABLES
                .hand
                .get_unchecked(pt.0 as usize)
                .get_unchecked(i as usize)
                .get_unchecked(c.0 as usize)
        }
    }
}

lazy_static! {
    static ref ZOBRIST_TABLES: Zobrist = {
        let mut zobrist = Zobrist {
            field: [[[Key(0); Color::NUM]; Square::NUM]; PieceType::NUM],
            hand: [[[Key(0); Color::NUM]; 19]; PieceType::NUM],
        };
        let seed = {
            let mut items = [0_u8; 32];
            for (i, item) in items.iter_mut().enumerate() {
                *item = (i + 1) as u8;
            }
            items
        };
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        for itemss in zobrist.field.iter_mut() {
            for items in itemss.iter_mut() {
                for item in items {
                    *item = Key(rng.gen::<u64>() & !1_u64); // Zobrist::COLOR is 1.
                }
            }
        }
        for itemss in zobrist.hand.iter_mut() {
            for items in itemss {
                for item in items {
                    *item = Key(rng.gen::<u64>() & !1_u64); // Zobrist::COLOR is 1.
                }
            }
        }
        zobrist
    };
}

#[derive(PartialEq, Eq)]
struct HuffmanCode {
    value: u8,
    bit_length: u8,
}

#[rustfmt::skip]
impl HuffmanCode {
    const EMPTY: HuffmanCode = HuffmanCode{value: 0b0, bit_length: 1};
    const B_PAWN: HuffmanCode = HuffmanCode{value: 0b1, bit_length: 4};
    const B_LANCE: HuffmanCode = HuffmanCode{value: 0b11, bit_length: 6};
    const B_KNIGHT: HuffmanCode = HuffmanCode{value: 0b111, bit_length: 6};
    const B_SILVER: HuffmanCode = HuffmanCode{value: 0b1011, bit_length: 6};
    const B_BISHOP: HuffmanCode = HuffmanCode{value: 0b1_1111, bit_length: 8};
    const B_ROOK: HuffmanCode = HuffmanCode{value: 0b11_1111, bit_length: 8};
    const B_GOLD: HuffmanCode = HuffmanCode{value: 0b1111, bit_length: 6};
    const B_PRO_PAWN: HuffmanCode = HuffmanCode{value: 0b1001, bit_length: 4};
    const B_PRO_LANCE: HuffmanCode = HuffmanCode{value: 0b10_0011, bit_length: 6};
    const B_PRO_KNIGHT: HuffmanCode = HuffmanCode{value: 0b10_0111, bit_length: 6};
    const B_PRO_SILVER: HuffmanCode = HuffmanCode{value: 0b10_1011, bit_length: 6};
    const B_HORSE: HuffmanCode = HuffmanCode{value: 0b1001_1111, bit_length: 8};
    const B_DRAGON: HuffmanCode = HuffmanCode{value: 0b1011_1111, bit_length: 8};
    const W_PAWN: HuffmanCode = HuffmanCode{value: 0b101, bit_length: 4};
    const W_LANCE: HuffmanCode = HuffmanCode{value: 0b1_0011, bit_length: 6};
    const W_KNIGHT: HuffmanCode = HuffmanCode{value: 0b1_0111, bit_length: 6};
    const W_SILVER: HuffmanCode = HuffmanCode{value: 0b1_1011, bit_length: 6};
    const W_BISHOP: HuffmanCode = HuffmanCode{value: 0b101_1111, bit_length: 8};
    const W_ROOK: HuffmanCode = HuffmanCode{value: 0b111_1111, bit_length: 8};
    const W_GOLD: HuffmanCode = HuffmanCode{value: 0b10_1111, bit_length: 6};
    const W_PRO_PAWN: HuffmanCode = HuffmanCode{value: 0b1101, bit_length: 4};
    const W_PRO_LANCE: HuffmanCode = HuffmanCode{value: 0b11_0011, bit_length: 6};
    const W_PRO_KNIGHT: HuffmanCode = HuffmanCode{value: 0b11_0111, bit_length: 6};
    const W_PRO_SILVER: HuffmanCode = HuffmanCode{value: 0b11_1011, bit_length: 6};
    const W_HORSE: HuffmanCode = HuffmanCode{value: 0b1101_1111, bit_length: 8};
    const W_DRAGON: HuffmanCode = HuffmanCode{value: 0b1111_1111, bit_length: 8};

    const B_HAND_PAWN: HuffmanCode = HuffmanCode{value: 0b0, bit_length: 3};
    const W_HAND_PAWN: HuffmanCode = HuffmanCode{value: 0b100, bit_length: 3};
    const B_HAND_LANCE: HuffmanCode = HuffmanCode{value: 0b1, bit_length: 5};
    const W_HAND_LANCE: HuffmanCode = HuffmanCode{value: 0b1_0001, bit_length: 5};
    const B_HAND_KNIGHT: HuffmanCode = HuffmanCode{value: 0b11, bit_length: 5};
    const W_HAND_KNIGHT: HuffmanCode = HuffmanCode{value: 0b1_0011, bit_length: 5};
    const B_HAND_SILVER: HuffmanCode = HuffmanCode{value: 0b101, bit_length: 5};
    const W_HAND_SILVER: HuffmanCode = HuffmanCode{value: 0b1_0101, bit_length: 5};
    const B_HAND_GOLD: HuffmanCode = HuffmanCode{value: 0b111, bit_length: 5};
    const W_HAND_GOLD: HuffmanCode = HuffmanCode{value: 0b1_0111, bit_length: 5};
    const B_HAND_BISHOP: HuffmanCode = HuffmanCode{value: 0b1_1111, bit_length: 7};
    const W_HAND_BISHOP: HuffmanCode = HuffmanCode{value: 0b101_1111, bit_length: 7};
    const B_HAND_ROOK: HuffmanCode = HuffmanCode{value: 0b11_1111, bit_length: 7};
    const W_HAND_ROOK: HuffmanCode = HuffmanCode{value: 0b111_1111, bit_length: 7};

    fn new(pc: Piece) -> HuffmanCode {
        match pc {
            Piece::EMPTY => HuffmanCode::EMPTY,
            Piece::B_PAWN => HuffmanCode::B_PAWN,
            Piece::B_LANCE => HuffmanCode::B_LANCE,
            Piece::B_KNIGHT => HuffmanCode::B_KNIGHT,
            Piece::B_SILVER => HuffmanCode::B_SILVER,
            Piece::B_BISHOP => HuffmanCode::B_BISHOP,
            Piece::B_ROOK => HuffmanCode::B_ROOK,
            Piece::B_GOLD => HuffmanCode::B_GOLD,
            Piece::B_PRO_PAWN => HuffmanCode::B_PRO_PAWN,
            Piece::B_PRO_LANCE => HuffmanCode::B_PRO_LANCE,
            Piece::B_PRO_KNIGHT => HuffmanCode::B_PRO_KNIGHT,
            Piece::B_PRO_SILVER => HuffmanCode::B_PRO_SILVER,
            Piece::B_HORSE => HuffmanCode::B_HORSE,
            Piece::B_DRAGON => HuffmanCode::B_DRAGON,
            Piece::W_PAWN => HuffmanCode::W_PAWN,
            Piece::W_LANCE => HuffmanCode::W_LANCE,
            Piece::W_KNIGHT => HuffmanCode::W_KNIGHT,
            Piece::W_SILVER => HuffmanCode::W_SILVER,
            Piece::W_BISHOP => HuffmanCode::W_BISHOP,
            Piece::W_ROOK => HuffmanCode::W_ROOK,
            Piece::W_GOLD => HuffmanCode::W_GOLD,
            Piece::W_PRO_PAWN => HuffmanCode::W_PRO_PAWN,
            Piece::W_PRO_LANCE => HuffmanCode::W_PRO_LANCE,
            Piece::W_PRO_KNIGHT => HuffmanCode::W_PRO_KNIGHT,
            Piece::W_PRO_SILVER => HuffmanCode::W_PRO_SILVER,
            Piece::W_HORSE => HuffmanCode::W_HORSE,
            Piece::W_DRAGON => HuffmanCode::W_DRAGON,
            _ => unreachable!(),
        }
    }
    fn new_from_color_and_hand_piece_type(c: Color, pt: PieceType) -> HuffmanCode {
        match (c, pt) {
            (Color::BLACK, PieceType::PAWN) => HuffmanCode::B_HAND_PAWN,
            (Color::BLACK, PieceType::LANCE) => HuffmanCode::B_HAND_LANCE,
            (Color::BLACK, PieceType::KNIGHT) => HuffmanCode::B_HAND_KNIGHT,
            (Color::BLACK, PieceType::SILVER) => HuffmanCode::B_HAND_SILVER,
            (Color::BLACK, PieceType::BISHOP) => HuffmanCode::B_HAND_BISHOP,
            (Color::BLACK, PieceType::ROOK) => HuffmanCode::B_HAND_ROOK,
            (Color::BLACK, PieceType::GOLD) => HuffmanCode::B_HAND_GOLD,
            (Color::WHITE, PieceType::PAWN) => HuffmanCode::W_HAND_PAWN,
            (Color::WHITE, PieceType::LANCE) => HuffmanCode::W_HAND_LANCE,
            (Color::WHITE, PieceType::KNIGHT) => HuffmanCode::W_HAND_KNIGHT,
            (Color::WHITE, PieceType::SILVER) => HuffmanCode::W_HAND_SILVER,
            (Color::WHITE, PieceType::BISHOP) => HuffmanCode::W_HAND_BISHOP,
            (Color::WHITE, PieceType::ROOK) => HuffmanCode::W_HAND_ROOK,
            (Color::WHITE, PieceType::GOLD) => HuffmanCode::W_HAND_GOLD,
            _ => unreachable!(),
        }
    }
}

impl std::convert::TryFrom<&HuffmanCode> for Piece {
    type Error = ();
    fn try_from(hc: &HuffmanCode) -> Result<Self, Self::Error> {
        match *hc {
            HuffmanCode::EMPTY => Ok(Piece::EMPTY),
            HuffmanCode::B_PAWN => Ok(Piece::B_PAWN),
            HuffmanCode::B_LANCE => Ok(Piece::B_LANCE),
            HuffmanCode::B_KNIGHT => Ok(Piece::B_KNIGHT),
            HuffmanCode::B_SILVER => Ok(Piece::B_SILVER),
            HuffmanCode::B_BISHOP => Ok(Piece::B_BISHOP),
            HuffmanCode::B_ROOK => Ok(Piece::B_ROOK),
            HuffmanCode::B_GOLD => Ok(Piece::B_GOLD),
            HuffmanCode::B_PRO_PAWN => Ok(Piece::B_PRO_PAWN),
            HuffmanCode::B_PRO_LANCE => Ok(Piece::B_PRO_LANCE),
            HuffmanCode::B_PRO_KNIGHT => Ok(Piece::B_PRO_KNIGHT),
            HuffmanCode::B_PRO_SILVER => Ok(Piece::B_PRO_SILVER),
            HuffmanCode::B_HORSE => Ok(Piece::B_HORSE),
            HuffmanCode::B_DRAGON => Ok(Piece::B_DRAGON),
            HuffmanCode::W_PAWN => Ok(Piece::W_PAWN),
            HuffmanCode::W_LANCE => Ok(Piece::W_LANCE),
            HuffmanCode::W_KNIGHT => Ok(Piece::W_KNIGHT),
            HuffmanCode::W_SILVER => Ok(Piece::W_SILVER),
            HuffmanCode::W_BISHOP => Ok(Piece::W_BISHOP),
            HuffmanCode::W_ROOK => Ok(Piece::W_ROOK),
            HuffmanCode::W_GOLD => Ok(Piece::W_GOLD),
            HuffmanCode::W_PRO_PAWN => Ok(Piece::W_PRO_PAWN),
            HuffmanCode::W_PRO_LANCE => Ok(Piece::W_PRO_LANCE),
            HuffmanCode::W_PRO_KNIGHT => Ok(Piece::W_PRO_KNIGHT),
            HuffmanCode::W_PRO_SILVER => Ok(Piece::W_PRO_SILVER),
            HuffmanCode::W_HORSE => Ok(Piece::W_HORSE),
            HuffmanCode::W_DRAGON => Ok(Piece::W_DRAGON),
            _ => Err(()),
        }
    }
}

type ColorAndPieceTypeForHand = (Color, PieceType);
impl std::convert::TryFrom<&HuffmanCode> for ColorAndPieceTypeForHand {
    type Error = ();
    fn try_from(hc: &HuffmanCode) -> Result<Self, Self::Error> {
        match *hc {
            HuffmanCode::B_HAND_PAWN => Ok((Color::BLACK, PieceType::PAWN)),
            HuffmanCode::W_HAND_PAWN => Ok((Color::BLACK, PieceType::PAWN)),
            HuffmanCode::B_HAND_LANCE => Ok((Color::BLACK, PieceType::LANCE)),
            HuffmanCode::W_HAND_LANCE => Ok((Color::BLACK, PieceType::LANCE)),
            HuffmanCode::B_HAND_KNIGHT => Ok((Color::BLACK, PieceType::KNIGHT)),
            HuffmanCode::W_HAND_KNIGHT => Ok((Color::BLACK, PieceType::KNIGHT)),
            HuffmanCode::B_HAND_SILVER => Ok((Color::BLACK, PieceType::SILVER)),
            HuffmanCode::W_HAND_SILVER => Ok((Color::BLACK, PieceType::SILVER)),
            HuffmanCode::B_HAND_GOLD => Ok((Color::BLACK, PieceType::GOLD)),
            HuffmanCode::W_HAND_GOLD => Ok((Color::BLACK, PieceType::GOLD)),
            HuffmanCode::B_HAND_BISHOP => Ok((Color::BLACK, PieceType::BISHOP)),
            HuffmanCode::W_HAND_BISHOP => Ok((Color::BLACK, PieceType::BISHOP)),
            HuffmanCode::B_HAND_ROOK => Ok((Color::BLACK, PieceType::ROOK)),
            HuffmanCode::W_HAND_ROOK => Ok((Color::BLACK, PieceType::ROOK)),
            _ => Err(()),
        }
    }
}

#[repr(C)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HuffmanCodedPosition {
    pub buf: [u8; 32],
    pub ply: i16,
}

impl HuffmanCodedPosition {
    pub fn from(pos: &Position) -> HuffmanCodedPosition {
        let mut hcp = HuffmanCodedPosition {
            buf: [0; 32],
            ply: pos.base.game_ply as i16,
        };
        let mut bs = BitStreamWriter::new(&mut hcp.buf);
        bs.put_bit_from_lsb(pos.side_to_move().0 as u8);
        bs.put_bits_from_lsb(pos.king_square(Color::BLACK).0 as u8, 7);
        bs.put_bits_from_lsb(pos.king_square(Color::WHITE).0 as u8, 7);
        for &sq in Square::ALL.iter() {
            let pc = pos.piece_on(sq);
            if pc == Piece::B_KING || pc == Piece::W_KING {
                continue;
            }
            let hc = HuffmanCode::new(pc);
            bs.put_bits_from_lsb(hc.value, hc.bit_length as usize);
        }
        for &c in Color::ALL.iter() {
            let hand = pos.hand(c);
            for &pt in PieceType::ALL_HAND.iter() {
                let hc = HuffmanCode::new_from_color_and_hand_piece_type(c, pt);
                for _ in 0..hand.num(pt) as usize {
                    bs.put_bits_from_lsb(hc.value, hc.bit_length as usize);
                }
            }
        }
        hcp
    }
}

struct BitStreamReader<'a> {
    slice: &'a [u8],
    current_index: usize,
    current_bit: usize,
}

struct BitStreamWriter<'a> {
    slice: &'a mut [u8],
    current_index: usize,
    current_bit: usize,
}

impl<'a> BitStreamReader<'a> {
    fn new(buf: &[u8]) -> BitStreamReader {
        BitStreamReader {
            slice: buf,
            current_index: 0,
            current_bit: 0,
        }
    }
    fn get_bit_from_lsb(&mut self) -> u8 {
        let bit = if (self.slice[self.current_index] & (1 << self.current_bit)) == 0 {
            0
        } else {
            1
        };
        self.current_bit += 1;
        if self.current_bit == 8 {
            self.current_index += 1;
            self.current_bit = 0;
        }
        bit
    }
    fn get_bits_from_lsb(&mut self, bit_length: usize) -> u8 {
        let mut bits = 0;
        for i in 0..bit_length {
            bits |= self.get_bit_from_lsb() << i;
        }
        bits
    }
}

impl<'a> BitStreamWriter<'a> {
    fn new(buf: &mut [u8]) -> BitStreamWriter {
        BitStreamWriter {
            slice: buf,
            current_index: 0,
            current_bit: 0,
        }
    }
    fn put_bit_from_lsb(&mut self, bit: u8) {
        debug_assert!(bit == 0 || bit == 1);
        self.slice[self.current_index] |= bit << self.current_bit;
        self.current_bit += 1;
        if self.current_bit == 8 {
            self.current_index += 1;
            self.current_bit = 0;
        }
    }
    fn put_bits_from_lsb(&mut self, bits: u8, bit_length: usize) {
        let mut bits = bits;
        for _ in 0..bit_length {
            let bit = bits & 1;
            bits >>= 1;
            self.put_bit_from_lsb(bit);
        }
    }
}

#[derive(Clone)]
pub struct EvalList(pub [[EvalIndex; 2]; LIST_NUM]);

impl EvalList {
    pub fn new(pos: &PositionBase) -> EvalList {
        let mut list = EvalList([[EvalIndex(0); 2]; LIST_NUM]);
        let mut index: usize = 0;
        let new_for_hand_pt = |pc: Piece, list: &mut EvalList, index: &mut usize| {
            let c = Color::new(pc);
            let pt = PieceType::new(pc);
            let opp_pc = pc.inverse();
            for i in 1..=pos.hand(c).num(pt) {
                list.set(
                    *index,
                    Color::BLACK,
                    EvalIndex(EvalIndex::new_hand(pc).0 + i as usize),
                );
                list.set(
                    *index,
                    Color::WHITE,
                    EvalIndex(EvalIndex::new_hand(opp_pc).0 + i as usize),
                );
                *index += 1;
            }
        };
        new_for_hand_pt(Piece::B_PAWN, &mut list, &mut index);
        new_for_hand_pt(Piece::W_PAWN, &mut list, &mut index);
        new_for_hand_pt(Piece::B_LANCE, &mut list, &mut index);
        new_for_hand_pt(Piece::W_LANCE, &mut list, &mut index);
        new_for_hand_pt(Piece::B_KNIGHT, &mut list, &mut index);
        new_for_hand_pt(Piece::W_KNIGHT, &mut list, &mut index);
        new_for_hand_pt(Piece::B_SILVER, &mut list, &mut index);
        new_for_hand_pt(Piece::W_SILVER, &mut list, &mut index);
        new_for_hand_pt(Piece::B_BISHOP, &mut list, &mut index);
        new_for_hand_pt(Piece::W_BISHOP, &mut list, &mut index);
        new_for_hand_pt(Piece::B_ROOK, &mut list, &mut index);
        new_for_hand_pt(Piece::W_ROOK, &mut list, &mut index);
        new_for_hand_pt(Piece::B_GOLD, &mut list, &mut index);
        new_for_hand_pt(Piece::W_GOLD, &mut list, &mut index);
        let bb = pos.occupied_bb() & !pos.pieces_p(PieceType::KING);
        for sq in bb {
            let pc = pos.piece_on(sq);
            let opp_pc = pc.inverse();
            list.set(
                index,
                Color::BLACK,
                EvalIndex(EvalIndex::new_board(pc).0 + sq.0 as usize),
            );
            list.set(
                index,
                Color::WHITE,
                EvalIndex(EvalIndex::new_board(opp_pc).0 + sq.inverse().0 as usize),
            );
            index += 1;
        }
        list
    }
    pub fn get(&self, list_index: usize, base_color: Color) -> EvalIndex {
        debug_assert!(list_index < self.0.len());
        debug_assert!((base_color.0 as usize) < self.0[0].len());
        unsafe {
            *self
                .0
                .get_unchecked(list_index)
                .get_unchecked(base_color.0 as usize)
        }
    }
    pub fn set(&mut self, list_index: usize, base_color: Color, eval_index: EvalIndex) {
        debug_assert!(list_index < self.0.len());
        debug_assert!((base_color.0 as usize) < self.0[0].len());
        unsafe {
            *self
                .0
                .get_unchecked_mut(list_index)
                .get_unchecked_mut(base_color.0 as usize) = eval_index;
        }
    }
}

#[derive(Clone)]
struct EvalIndexToEvalListIndex([usize; EvalIndex::FE_END.0]);

impl EvalIndexToEvalListIndex {
    fn new(eval_list: &EvalList) -> EvalIndexToEvalListIndex {
        let mut eval_index_to_eval_list_index = EvalIndexToEvalListIndex([0; EvalIndex::FE_END.0]);
        for (eval_list_index, eval_indices) in eval_list.0.iter().enumerate() {
            let eval_index_black = eval_indices[0];
            eval_index_to_eval_list_index.0[eval_index_black.0] = eval_list_index;
        }
        eval_index_to_eval_list_index
    }
    fn get(&self, eval_index: EvalIndex) -> usize {
        debug_assert!(eval_index.0 < EvalIndex::FE_END.0);
        unsafe { *self.0.get_unchecked(eval_index.0) }
    }
    fn set(&mut self, eval_index: EvalIndex, list_index: usize) {
        debug_assert!(eval_index.0 < EvalIndex::FE_END.0);
        unsafe { *self.0.get_unchecked_mut(eval_index.0) = list_index }
    }
}

#[derive(Clone)]
pub struct StateInfo {
    material: Value,
    plies_from_null: i32,
    continuous_checks: [i32; Color::NUM],
    board_key: Key,
    hand_key: Key,
    hand_of_side_to_move: Hand,
    checkers_bb: Bitboard,
    captured_piece: Piece,
    check_info: CheckInfo,
    changed_eval_index: ChangedEvalIndex,
    changed_eval_index_captured: ChangedEvalIndex,
}

impl StateInfo {
    fn new() -> StateInfo {
        StateInfo {
            material: Value(0),
            plies_from_null: 0,
            continuous_checks: [0, 0],
            board_key: Key(0),
            hand_key: Key(0),
            hand_of_side_to_move: Hand(0),
            checkers_bb: Bitboard::ZERO,
            captured_piece: Piece::EMPTY,
            check_info: CheckInfo::ZERO,
            changed_eval_index: ChangedEvalIndex::ZERO,
            changed_eval_index_captured: ChangedEvalIndex::ZERO,
        }
    }
    unsafe fn new_from_old_state(old_state: &StateInfo) -> StateInfo {
        StateInfo {
            material: old_state.material,
            plies_from_null: old_state.plies_from_null,
            continuous_checks: old_state.continuous_checks,
            board_key: std::mem::uninitialized(),
            hand_key: std::mem::uninitialized(),
            hand_of_side_to_move: std::mem::uninitialized(),
            checkers_bb: std::mem::uninitialized(),
            captured_piece: std::mem::uninitialized(),
            check_info: std::mem::uninitialized(),
            changed_eval_index: std::mem::uninitialized(),
            changed_eval_index_captured: std::mem::uninitialized(),
        }
    }
    fn new_from_position(pos: &PositionBase) -> StateInfo {
        let us = pos.side_to_move();
        let them = us.inverse();
        let king_sq = pos.king_square(us);
        StateInfo {
            material: StateInfo::new_material(pos),
            plies_from_null: 0,
            continuous_checks: [0, 0],
            board_key: StateInfo::new_board_key(pos),
            hand_key: StateInfo::new_hand_key(pos),
            hand_of_side_to_move: pos.hand(us),
            checkers_bb: pos.attackers_to_except_king(them, king_sq, &pos.occupied_bb()),
            captured_piece: Piece::EMPTY,
            check_info: CheckInfo::new(&pos),
            changed_eval_index: ChangedEvalIndex::ZERO,
            changed_eval_index_captured: ChangedEvalIndex::ZERO,
        }
    }
    fn new_material(pos: &PositionBase) -> Value {
        let mut val = Value(0);
        for &pt in [
            PieceType::PAWN,
            PieceType::LANCE,
            PieceType::KNIGHT,
            PieceType::SILVER,
            PieceType::BISHOP,
            PieceType::ROOK,
            PieceType::GOLD,
            PieceType::PRO_PAWN,
            PieceType::PRO_LANCE,
            PieceType::PRO_KNIGHT,
            PieceType::PRO_SILVER,
            PieceType::HORSE,
            PieceType::DRAGON,
        ]
        .iter()
        {
            let num = pos.pieces_cp(Color::BLACK, pt).count_ones() as i32
                - pos.pieces_cp(Color::WHITE, pt).count_ones() as i32;
            val += Value(num * piece_type_value(pt).0);
        }
        for &pt in PieceType::ALL_HAND.iter() {
            let num = pos.hand(Color::BLACK).num(pt) as i32 - pos.hand(Color::WHITE).num(pt) as i32;
            val += Value(num * piece_type_value(pt).0);
        }
        val
    }
    fn new_board_key(pos: &PositionBase) -> Key {
        let mut key = Key(0);
        for sq in pos.occupied_bb() {
            let pc = pos.piece_on(sq);
            let (pt, c) = (PieceType::new(pc), Color::new(pc));
            key ^= Zobrist::get_field(pt, sq, c);
        }
        if pos.side_to_move() == Color::WHITE {
            key ^= Zobrist::COLOR;
        }
        key
    }
    fn new_hand_key(pos: &PositionBase) -> Key {
        let mut key = Key(0);
        for &pt in PieceType::ALL_HAND.iter() {
            for &c in Color::ALL.iter() {
                for i in 1..=pos.hand(c).num(pt) {
                    key ^= Zobrist::get_hand(pt, i, c);
                }
            }
        }
        key
    }
    #[allow(dead_code)]
    fn ci(&self) -> &CheckInfo {
        &self.check_info
    }
    fn key(&self) -> Key {
        self.board_key ^ self.hand_key
    }
    fn continuous_check(&self, c: Color) -> i32 {
        debug_assert!(0 <= c.0 && (c.0 as usize) < self.continuous_checks.len());
        unsafe { *self.continuous_checks.get_unchecked(c.0 as usize) }
    }
    fn is_capture_move(&self) -> bool {
        self.captured_piece != Piece::EMPTY
    }
    #[allow(dead_code)]
    pub const ZERO: StateInfo = StateInfo {
        material: Value(0),
        plies_from_null: 0,
        continuous_checks: [0, 0],
        board_key: Key(0),
        hand_key: Key(0),
        hand_of_side_to_move: Hand(0),
        checkers_bb: Bitboard::ZERO,
        captured_piece: Piece::EMPTY,
        check_info: CheckInfo::ZERO,
        changed_eval_index: ChangedEvalIndex::ZERO,
        changed_eval_index_captured: ChangedEvalIndex::ZERO,
    };
}

#[derive(Clone)]
pub struct PositionBase {
    board: [Piece; Square::NUM],
    by_type_bb: [Bitboard; PieceType::NUM],
    by_color_bb: [Bitboard; Color::NUM],
    golds_bb: Bitboard,
    hands: [Hand; Color::NUM],
    game_ply: i32,
    king_squares: [Square; Color::NUM],
    side_to_move: Color,
}

impl PositionBase {
    pub fn new_from_sfen_args(sfen_slice: &[&str]) -> Result<PositionBase, SfenError> {
        if sfen_slice.len() < 4 {
            return Err(SfenError::InvalidNumberOfSections {
                sections: sfen_slice.len(),
            });
        }
        let board_str = sfen_slice[0];
        let side_to_move_str = sfen_slice[1];
        let hands_str = sfen_slice[2];
        let game_ply_str = sfen_slice[3];
        let mut pos = PositionBase {
            board: [Piece::EMPTY; Square::NUM],
            by_type_bb: [Bitboard::ZERO; PieceType::NUM],
            by_color_bb: [Bitboard::ZERO; Color::NUM],
            golds_bb: Bitboard::ZERO,
            hands: [Hand(0); Color::NUM],
            game_ply: 0,
            king_squares: [Square(0), Square(0)],
            side_to_move: Color::BLACK,
        };
        let rank_str_vec: Vec<&str> = board_str.split('/').collect();
        if rank_str_vec.len() != Rank::NUM {
            return Err(SfenError::InvalidNumberOfRanks {
                ranks: rank_str_vec.len(),
            });
        }
        for (rank_idx, rank) in Rank::ALL_FROM_UPPER.iter().enumerate() {
            let rank_str = rank_str_vec[rank_idx as usize];
            let mut file_idx: usize = 0;
            let re = regex::Regex::new(r"(\d+|\+?[[:alpha:]])").unwrap();
            for cap in re.captures_iter(rank_str) {
                if file_idx >= File::NUM {
                    return Err(SfenError::InvalidNumberOfFiles { files: file_idx });
                }
                let token: &str = &cap[0];
                if let Ok(digit) = token.to_string().parse::<i64>() {
                    if digit <= 0
                        || (Rank::NUM as i64) < digit
                        || (Rank::NUM as i64) < (file_idx as i64) + digit
                    {
                        return Err(SfenError::InvalidNumberOfEmptySquares {
                            empty_squares: digit,
                        });
                    }
                    file_idx += digit as usize;
                } else if let Some(pc) = Piece::new_from_str(token) {
                    let pt = PieceType::new(pc);
                    let sq = Square::new(File::ALL_FROM_LEFT[file_idx], *rank);
                    let c = Color::new(pc);
                    pos.board[sq.0 as usize] = pc;
                    pos.by_type_bb[PieceType::OCCUPIED.0 as usize].set(sq);
                    pos.by_type_bb[pt.0 as usize].set(sq);
                    pos.by_color_bb[c.0 as usize].set(sq);
                    file_idx += 1;
                } else {
                    return Err(SfenError::InvalidPieceCharactors {
                        chars: token.to_string(),
                    });
                }
            }
        }
        pos.set_golds_bb();
        for c in Color::ALL.iter() {
            let mut bb = pos.pieces_cp(*c, PieceType::KING);
            match bb.pop_lsb() {
                Some(sq) => pos.king_squares[c.0 as usize] = sq,
                None => return Err(SfenError::KingIsNothing { c: *c }),
            }
        }
        match side_to_move_str {
            "b" => pos.side_to_move = Color::BLACK,
            "w" => pos.side_to_move = Color::WHITE,
            _ => {
                return Err(SfenError::InvalidSideToMoveCharactors {
                    chars: side_to_move_str.to_string(),
                });
            }
        }
        if hands_str != "-" {
            let mut hand_num: i64 = 1;
            let re = regex::Regex::new(r"(\d+|[[:alpha:]])").unwrap();
            for cap in re.captures_iter(hands_str) {
                let token: &str = &cap[0];
                if let Ok(digit) = token.to_string().parse::<i64>() {
                    if digit <= 0 {
                        return Err(SfenError::InvalidNumberOfHandPieces { number: digit });
                    }
                    hand_num = digit;
                } else if let Some(pc) = Piece::new_hand_piece_from_str(token) {
                    let pt = PieceType::new(pc);
                    let c = Color::new(pc);
                    match pt {
                        PieceType::PAWN if 18 < hand_num => {
                            return Err(SfenError::InvalidNumberOfPawns { number: hand_num });
                        }
                        PieceType::LANCE if 4 < hand_num => {
                            return Err(SfenError::InvalidNumberOfLances { number: hand_num });
                        }
                        PieceType::KNIGHT if 4 < hand_num => {
                            return Err(SfenError::InvalidNumberOfKnights { number: hand_num });
                        }
                        PieceType::SILVER if 4 < hand_num => {
                            return Err(SfenError::InvalidNumberOfSilvers { number: hand_num });
                        }
                        PieceType::GOLD if 4 < hand_num => {
                            return Err(SfenError::InvalidNumberOfGolds { number: hand_num });
                        }
                        PieceType::BISHOP if 2 < hand_num => {
                            return Err(SfenError::InvalidNumberOfBishops { number: hand_num });
                        }
                        PieceType::ROOK if 2 < hand_num => {
                            return Err(SfenError::InvalidNumberOfRooks { number: hand_num });
                        }
                        _ => {
                            if pos.hands[c.0 as usize].exist(pt) {
                                return Err(SfenError::SameHandPieceTwice { pt });
                            }
                            pos.hands[c.0 as usize].set(pt, hand_num as u32);
                            hand_num = 1; // reset hand_num
                        }
                    };
                } else {
                    return Err(SfenError::InvalidHandPieceCharactors {
                        chars: token.to_string(),
                    });
                }
            }
            if hand_num != 1 {
                return Err(SfenError::InvalidHandPieceCharactors {
                    chars: "".to_string(),
                });
            }
        }
        match game_ply_str.to_string().parse::<i32>() {
            Ok(game_ply) if 1 <= game_ply => pos.game_ply = game_ply,
            Ok(_) | Err(_) => {
                return Err(SfenError::InvalidGamePly {
                    chars: game_ply_str.to_string(),
                });
            }
        }
        fn check_pieces(pos: &PositionBase, pts: &[PieceType], max: i64) -> Result<(), SfenError> {
            let number = i64::from(
                pts.iter()
                    .fold(0, |sum, &pt| sum + pos.pieces_p(pt).count_ones())
                    + pos.hands.iter().fold(0, |sum, hand| sum + hand.num(pts[0])),
            );
            if number <= max {
                Ok(())
            } else {
                match pts[0] {
                    PieceType::PAWN => Err(SfenError::InvalidNumberOfPawns { number }),
                    PieceType::LANCE => Err(SfenError::InvalidNumberOfLances { number }),
                    PieceType::KNIGHT => Err(SfenError::InvalidNumberOfKnights { number }),
                    PieceType::SILVER => Err(SfenError::InvalidNumberOfSilvers { number }),
                    PieceType::GOLD => Err(SfenError::InvalidNumberOfGolds { number }),
                    PieceType::BISHOP => Err(SfenError::InvalidNumberOfBishops { number }),
                    PieceType::ROOK => Err(SfenError::InvalidNumberOfRooks { number }),
                    _ => unreachable!(),
                }
            }
        }
        check_pieces(&pos, &[PieceType::PAWN, PieceType::PRO_PAWN], 18)?;
        check_pieces(&pos, &[PieceType::LANCE, PieceType::PRO_LANCE], 4)?;
        check_pieces(&pos, &[PieceType::KNIGHT, PieceType::PRO_KNIGHT], 4)?;
        check_pieces(&pos, &[PieceType::SILVER, PieceType::PRO_SILVER], 4)?;
        check_pieces(&pos, &[PieceType::GOLD], 4)?;
        check_pieces(&pos, &[PieceType::BISHOP, PieceType::HORSE], 2)?;
        check_pieces(&pos, &[PieceType::ROOK, PieceType::DRAGON], 2)?;
        Ok(pos)
    }
    pub fn new_from_huffman_coded_position(
        hcp: &HuffmanCodedPosition,
    ) -> Result<PositionBase, u32> {
        let mut bs = BitStreamReader::new(&hcp.buf);
        let mut pos = PositionBase {
            board: [Piece::EMPTY; Square::NUM],
            by_type_bb: [Bitboard::ZERO; PieceType::NUM],
            by_color_bb: [Bitboard::ZERO; Color::NUM],
            golds_bb: Bitboard::ZERO,
            hands: [Hand(0); Color::NUM],
            game_ply: 0,
            king_squares: [Square(0), Square(0)],
            side_to_move: Color::BLACK,
        };
        pos.side_to_move = Color(i32::from(bs.get_bit_from_lsb()));
        pos.king_squares[Color::BLACK.0 as usize] = {
            let val = bs.get_bits_from_lsb(7);
            Square(i32::from(val))
        };
        pos.king_squares[Color::WHITE.0 as usize] = {
            let val = bs.get_bits_from_lsb(7);
            Square(i32::from(val))
        };
        pos.put_piece(Piece::B_KING, pos.king_square(Color::BLACK));
        pos.put_piece(Piece::W_KING, pos.king_square(Color::WHITE));
        for &sq in Square::ALL.iter() {
            if sq == pos.king_square(Color::BLACK) || sq == pos.king_square(Color::WHITE) {
                continue;
            }
            let mut hc = HuffmanCode {
                value: 0,
                bit_length: 0,
            };
            loop {
                hc.value |= bs.get_bit_from_lsb() << hc.bit_length;
                hc.bit_length += 1;
                if let Ok(pc) = Piece::try_from(&hc) {
                    if pc != Piece::EMPTY {
                        pos.put_piece(pc, sq);
                    }
                    break;
                }
                if hc.bit_length >= 8 {
                    return Err(line!());
                }
            }
        }
        while bs.slice.len() != bs.current_index {
            let mut hc = HuffmanCode {
                value: 0,
                bit_length: 0,
            };
            loop {
                hc.value |= bs.get_bit_from_lsb() << hc.bit_length;
                hc.bit_length += 1;
                if let Ok((c, pt)) = ColorAndPieceTypeForHand::try_from(&hc) {
                    pos.hands[c.0 as usize].plus_one(pt);
                    break;
                }
                if hc.bit_length >= 7 {
                    return Err(line!());
                }
            }
        }
        pos.set_golds_bb();
        pos.game_ply = i32::from(hcp.ply);
        Ok(pos)
    }
    fn pieces_c(&self, c: Color) -> Bitboard {
        debug_assert!((c.0 as usize) < Color::NUM);
        unsafe { *self.by_color_bb.get_unchecked(c.0 as usize) }
    }
    fn pieces_p(&self, pt: PieceType) -> Bitboard {
        debug_assert!((pt.0 as usize) < PieceType::NUM);
        unsafe { *self.by_type_bb.get_unchecked(pt.0 as usize) }
    }
    fn pieces_cp(&self, c: Color, pt: PieceType) -> Bitboard {
        self.pieces_c(c) & self.pieces_p(pt)
    }
    fn pieces_pp(&self, pt0: PieceType, pt1: PieceType) -> Bitboard {
        self.pieces_p(pt0) | self.pieces_p(pt1)
    }
    fn pieces_cpp(&self, c: Color, pt0: PieceType, pt1: PieceType) -> Bitboard {
        self.pieces_c(c) & self.pieces_pp(pt0, pt1)
    }
    fn pieces_ppp(&self, pt0: PieceType, pt1: PieceType, pt2: PieceType) -> Bitboard {
        self.pieces_pp(pt0, pt1) | self.pieces_p(pt2)
    }
    fn pieces_cppp(&self, c: Color, pt0: PieceType, pt1: PieceType, pt2: PieceType) -> Bitboard {
        self.pieces_c(c) & self.pieces_ppp(pt0, pt1, pt2)
    }
    fn pieces_pppp(
        &self,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
    ) -> Bitboard {
        self.pieces_ppp(pt0, pt1, pt2) | self.pieces_p(pt3)
    }
    fn pieces_cpppp(
        &self,
        c: Color,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
    ) -> Bitboard {
        self.pieces_c(c) & self.pieces_pppp(pt0, pt1, pt2, pt3)
    }
    fn pieces_ppppp(
        &self,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
        pt4: PieceType,
    ) -> Bitboard {
        self.pieces_pppp(pt0, pt1, pt2, pt3) | self.pieces_p(pt4)
    }
    pub fn pieces_golds(&self) -> Bitboard {
        debug_assert_eq!(
            self.golds_bb,
            self.pieces_ppppp(
                PieceType::GOLD,
                PieceType::PRO_PAWN,
                PieceType::PRO_LANCE,
                PieceType::PRO_KNIGHT,
                PieceType::PRO_SILVER
            )
        );
        self.golds_bb
    }
    fn set_golds_bb(&mut self) {
        self.golds_bb = self.pieces_ppppp(
            PieceType::GOLD,
            PieceType::PRO_PAWN,
            PieceType::PRO_LANCE,
            PieceType::PRO_KNIGHT,
            PieceType::PRO_SILVER,
        );
    }
    pub fn piece_on(&self, sq: Square) -> Piece {
        debug_assert!((sq.0 as usize) < Square::NUM);
        unsafe { *self.board.get_unchecked(sq.0 as usize) }
    }
    pub fn occupied_bb(&self) -> Bitboard {
        unsafe {
            *self
                .by_type_bb
                .get_unchecked(PieceType::OCCUPIED.0 as usize)
        }
    }
    pub fn empty_bb(&self) -> Bitboard {
        Bitboard::ALL & !self.occupied_bb()
    }
    pub fn hand(&self, c: Color) -> Hand {
        debug_assert!((c.0 as usize) < Color::NUM);
        unsafe { *self.hands.get_unchecked(c.0 as usize) }
    }
    pub fn side_to_move(&self) -> Color {
        self.side_to_move
    }
    pub fn king_square(&self, c: Color) -> Square {
        debug_assert!((c.0 as usize) < Color::NUM);
        unsafe { *self.king_squares.get_unchecked(c.0 as usize) }
    }
    fn xor_bbs(&mut self, c: Color, pt: PieceType, sq: Square) {
        debug_assert!(0 <= c.0 && (c.0 as usize) < Color::NUM);
        debug_assert!(0 <= pt.0 && (pt.0 as usize) < PieceType::NUM);
        debug_assert!(0 <= sq.0 && (sq.0 as usize) < Square::NUM);
        unsafe {
            self.by_type_bb
                .get_unchecked_mut(PieceType::OCCUPIED.0 as usize)
                .xor(sq);
            self.by_type_bb.get_unchecked_mut(pt.0 as usize).xor(sq);
            self.by_color_bb.get_unchecked_mut(c.0 as usize).xor(sq);
        }
    }
    fn put_piece(&mut self, pc: Piece, sq: Square) {
        debug_assert!(!self.pieces_p(PieceType::new(pc)).is_set(sq));
        debug_assert!(!self.pieces_c(Color::new(pc)).is_set(sq));
        debug_assert!(!self.occupied_bb().is_set(sq));
        //debug_assert_eq!(self.piece_on(sq), Piece::EMPTY);
        self.xor_bbs(Color::new(pc), PieceType::new(pc), sq);
        unsafe {
            *self.board.get_unchecked_mut(sq.0 as usize) = pc;
        }
    }
    fn remove_piece(&mut self, pc: Piece, sq: Square) {
        debug_assert!(self.pieces_p(PieceType::new(pc)).is_set(sq));
        debug_assert!(self.pieces_c(Color::new(pc)).is_set(sq));
        debug_assert!(self.occupied_bb().is_set(sq));
        debug_assert_eq!(self.piece_on(sq), pc);
        self.xor_bbs(Color::new(pc), PieceType::new(pc), sq);
        unsafe {
            *self.board.get_unchecked_mut(sq.0 as usize) = Piece::EMPTY;
        }
    }
    fn exchange_pieces(&mut self, pc_new: Piece, sq: Square) {
        let pt_new = PieceType::new(pc_new);
        let pc_old = self.piece_on(sq);
        let pt_old = PieceType::new(pc_old);
        let color_old = Color::new(pc_old);
        let color_new = color_old.inverse();
        debug_assert!(self.pieces_p(pt_old).is_set(sq));
        debug_assert!(self.pieces_c(color_old).is_set(sq));
        unsafe {
            self.by_type_bb.get_unchecked_mut(pt_old.0 as usize).xor(sq);
            self.by_type_bb.get_unchecked_mut(pt_new.0 as usize).xor(sq);
            self.by_color_bb
                .get_unchecked_mut(color_old.0 as usize)
                .xor(sq);
            self.by_color_bb
                .get_unchecked_mut(color_new.0 as usize)
                .xor(sq);
            *self.board.get_unchecked_mut(sq.0 as usize) = pc_new;
        }
        debug_assert!(self.pieces_p(pt_new).is_set(sq));
        debug_assert!(self.pieces_c(color_new).is_set(sq));
    }
    pub fn attackers_to(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        let opp = color_of_attackers.inverse();
        let golds = self.pieces_golds();
        ((ATTACK_TABLE.pawn.attack(opp, to) & self.pieces_p(PieceType::PAWN))
            | (ATTACK_TABLE.lance.attack(opp, to, occupied) & self.pieces_p(PieceType::LANCE))
            | (ATTACK_TABLE.knight.attack(opp, to) & self.pieces_p(PieceType::KNIGHT))
            | (ATTACK_TABLE.silver.attack(opp, to)
                & (self.pieces_ppp(PieceType::SILVER, PieceType::KING, PieceType::DRAGON)))
            | (ATTACK_TABLE.gold.attack(opp, to)
                & (golds | self.pieces_pp(PieceType::KING, PieceType::HORSE)))
            | (ATTACK_TABLE.bishop.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::BISHOP, PieceType::HORSE)))
            | (ATTACK_TABLE.rook.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::ROOK, PieceType::DRAGON))))
            & self.pieces_c(color_of_attackers)
    }
    pub fn attackers_to_except_king(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        let opp = color_of_attackers.inverse();
        let golds = self.pieces_golds();
        ((ATTACK_TABLE.pawn.attack(opp, to) & self.pieces_p(PieceType::PAWN))
            | (ATTACK_TABLE.lance.attack(opp, to, occupied) & self.pieces_p(PieceType::LANCE))
            | (ATTACK_TABLE.knight.attack(opp, to) & self.pieces_p(PieceType::KNIGHT))
            | (ATTACK_TABLE.silver.attack(opp, to)
                & (self.pieces_pp(PieceType::SILVER, PieceType::DRAGON)))
            | (ATTACK_TABLE.gold.attack(opp, to) & (golds | self.pieces_p(PieceType::HORSE)))
            | (ATTACK_TABLE.bishop.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::BISHOP, PieceType::HORSE)))
            | (ATTACK_TABLE.rook.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::ROOK, PieceType::DRAGON))))
            & self.pieces_c(color_of_attackers)
    }
    pub fn attackers_to_except_king_lance_pawn(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        let opp = color_of_attackers.inverse();
        let golds = self.pieces_golds();
        ((ATTACK_TABLE.knight.attack(opp, to) & self.pieces_p(PieceType::KNIGHT))
            | (ATTACK_TABLE.silver.attack(opp, to)
                & (self.pieces_pp(PieceType::SILVER, PieceType::DRAGON)))
            | (ATTACK_TABLE.gold.attack(opp, to) & (golds | self.pieces_p(PieceType::HORSE)))
            | (ATTACK_TABLE.bishop.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::BISHOP, PieceType::HORSE)))
            | (ATTACK_TABLE.rook.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::ROOK, PieceType::DRAGON))))
            & self.pieces_c(color_of_attackers)
    }
    pub fn attackers_to_both_color(&self, to: Square, occupied: &Bitboard) -> Bitboard {
        let golds = self.pieces_golds();
        (((ATTACK_TABLE.pawn.attack(Color::BLACK, to) & self.pieces_p(PieceType::PAWN))
            | (ATTACK_TABLE.lance.attack(Color::BLACK, to, occupied)
                & self.pieces_p(PieceType::LANCE))
            | (ATTACK_TABLE.knight.attack(Color::BLACK, to) & self.pieces_p(PieceType::KNIGHT))
            | (ATTACK_TABLE.silver.attack(Color::BLACK, to) & self.pieces_p(PieceType::SILVER))
            | (ATTACK_TABLE.gold.attack(Color::BLACK, to) & golds))
            & self.pieces_c(Color::WHITE))
            | (((ATTACK_TABLE.pawn.attack(Color::WHITE, to) & self.pieces_p(PieceType::PAWN))
                | (ATTACK_TABLE.lance.attack(Color::WHITE, to, occupied)
                    & self.pieces_p(PieceType::LANCE))
                | (ATTACK_TABLE.knight.attack(Color::WHITE, to)
                    & self.pieces_p(PieceType::KNIGHT))
                | (ATTACK_TABLE.silver.attack(Color::WHITE, to)
                    & self.pieces_p(PieceType::SILVER))
                | (ATTACK_TABLE.gold.attack(Color::WHITE, to) & golds))
                & self.pieces_c(Color::BLACK))
            | (ATTACK_TABLE.bishop.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::BISHOP, PieceType::HORSE)))
            | (ATTACK_TABLE.rook.magic(to).attack(occupied)
                & (self.pieces_pp(PieceType::ROOK, PieceType::DRAGON)))
            | (ATTACK_TABLE.king.attack(to)
                & (self.pieces_ppp(PieceType::KING, PieceType::HORSE, PieceType::DRAGON)))
    }
    // sliders can be self.pieces_c(Color)
    // return (blockers of both colors, pinners)
    pub fn slider_blockers_and_pinners(
        &self,
        sliders: &Bitboard,
        color_of_sliders: Color,
        sq: Square,
    ) -> (Bitboard, Bitboard) {
        let opp_of_sliders = color_of_sliders.inverse();
        let mut blockers = Bitboard::ZERO;
        let mut pinners = Bitboard::ZERO;
        let snipers = ((ATTACK_TABLE.lance.pseudo_attack(opp_of_sliders, sq)
            & self.pieces_p(PieceType::LANCE))
            | (ATTACK_TABLE.bishop.magic(sq).pseudo_attack()
                & self.pieces_pp(PieceType::BISHOP, PieceType::HORSE))
            | (ATTACK_TABLE.rook.magic(sq).pseudo_attack()
                & self.pieces_pp(PieceType::ROOK, PieceType::DRAGON)))
            & *sliders;

        for sq_of_sniper in snipers {
            let pseudo_blockers = Bitboard::between_mask(sq, sq_of_sniper) & self.occupied_bb();
            if pseudo_blockers.count_ones() == 1 {
                blockers |= pseudo_blockers;
                if (pseudo_blockers & self.pieces_c(Color::new(self.piece_on(sq)))).to_bool() {
                    pinners.set(sq_of_sniper);
                }
            }
        }
        (blockers, pinners)
    }
    pub fn to_csa_string(&self) -> String {
        let mut s: String = "".to_string();
        s += "'  9  8  7  6  5  4  3  2  1\n";
        for (i, rank) in Rank::ALL_FROM_UPPER.iter().enumerate() {
            s += "P";
            s += &(i + 1).to_string();
            for file in File::ALL_FROM_LEFT.iter() {
                let sq = Square::new(*file, *rank);
                s += self.piece_on(sq).to_csa_str();
            }
            s += "\n";
        }
        for c in [Color::BLACK, Color::WHITE].iter() {
            for pt in [
                PieceType::PAWN,
                PieceType::LANCE,
                PieceType::KNIGHT,
                PieceType::SILVER,
                PieceType::GOLD,
                PieceType::BISHOP,
                PieceType::ROOK,
            ]
            .iter()
            {
                let hand_num = self.hand(*c).num(*pt);
                if hand_num != 0 {
                    s += if *c == Color::BLACK { "P+" } else { "P-" };
                    for _ in 0..hand_num {
                        s += "00";
                        s += pt.to_csa_str();
                    }
                    s += "\n";
                }
            }
        }
        s += if self.side_to_move == Color::BLACK {
            "+\n"
        } else {
            "-\n"
        };
        s
    }
    pub fn print(&self) {
        println!("{}", self.to_csa_string());
    }
    pub fn to_sfen(&self) -> String {
        let mut s = "".to_string();
        for rank in Rank::ALL_FROM_UPPER.iter() {
            let mut empty_squares = 0;
            if s != "" {
                s += "/";
            }
            for file in File::ALL_FROM_LEFT.iter() {
                let sq = Square::new(*file, *rank);
                let pc = self.piece_on(sq);
                if pc == Piece::EMPTY {
                    empty_squares += 1;
                } else {
                    if empty_squares != 0 {
                        s += &empty_squares.to_string();
                    }
                    s += &pc.to_usi_str();
                    empty_squares = 0; // reset empty_squares
                }
            }
            if empty_squares != 0 {
                s += &empty_squares.to_string();
            }
        }
        match self.side_to_move {
            Color::BLACK => s += " b ",
            Color::WHITE => s += " w ",
            _ => unreachable!(),
        }
        if self.hand(Color::BLACK).0 == 0 && self.hand(Color::WHITE).0 == 0 {
            s += "-";
        } else {
            for c in Color::ALL_FROM_BLACK.iter() {
                for pt in PieceType::ALL_HAND_FOR_SFEN.iter() {
                    let num = self.hand(*c).num(*pt);
                    if 2 <= num {
                        s += &num.to_string();
                    }
                    if num != 0 {
                        let pc = Piece::new(*c, *pt);
                        s += &pc.to_usi_str();
                    }
                }
            }
        }
        s += " ";
        s += &self.game_ply.to_string();
        s
    }
}

pub struct Position {
    pub base: PositionBase,
    eval_list: EvalList,
    eval_index_to_eval_list_index: EvalIndexToEvalListIndex,
    states: Vec<StateInfo>,
    nodes: Arc<AtomicI64>,
}

impl Position {
    pub fn new() -> Position {
        Position::new_from_sfen(START_SFEN).unwrap()
    }
    pub fn new_from_sfen(sfen: &str) -> Result<Position, SfenError> {
        Self::new_from_sfen_args(sfen.split_whitespace().collect::<Vec<&str>>().as_slice())
    }
    pub fn new_from_sfen_args(sfen_slice: &[&str]) -> Result<Position, SfenError> {
        match PositionBase::new_from_sfen_args(sfen_slice) {
            Ok(base) => {
                let state = StateInfo::new_from_position(&base);
                let eval_list = EvalList::new(&base);
                let eval_index_to_eval_list_index = EvalIndexToEvalListIndex::new(&eval_list);
                let mut pos = Position {
                    base,
                    eval_list,
                    eval_index_to_eval_list_index,
                    states: Vec::new(),
                    nodes: Arc::new(AtomicI64::new(0)),
                };
                pos.init_states_and_push(state);
                debug_assert!(pos.is_ok());
                Ok(pos)
            }
            Err(sfen_error) => Err(sfen_error),
        }
    }
    pub fn new_from_huffman_coded_position(hcp: &HuffmanCodedPosition) -> Result<Position, u32> {
        match PositionBase::new_from_huffman_coded_position(hcp) {
            Ok(base) => {
                let state = StateInfo::new_from_position(&base);
                let eval_list = EvalList::new(&base);
                let eval_index_to_eval_list_index = EvalIndexToEvalListIndex::new(&eval_list);
                let mut pos = Position {
                    base,
                    eval_list,
                    eval_index_to_eval_list_index,
                    states: Vec::new(),
                    nodes: Arc::new(AtomicI64::new(0)),
                };
                pos.init_states_and_push(state);
                debug_assert!(pos.is_ok());
                Ok(pos)
            }
            Err(err) => Err(err),
        }
    }
    pub fn new_from_position(pos: &Position, nodes: Arc<AtomicI64>) -> Position {
        let mut p = Position {
            base: pos.base.clone(),
            eval_list: pos.eval_list.clone(),
            eval_index_to_eval_list_index: pos.eval_index_to_eval_list_index.clone(),
            states: pos.states.clone(),
            nodes,
        };
        p.reserve_states();
        p
    }
    #[inline]
    pub fn pieces_c(&self, c: Color) -> Bitboard {
        self.base.pieces_c(c)
    }
    #[inline]
    pub fn pieces_p(&self, pt: PieceType) -> Bitboard {
        self.base.pieces_p(pt)
    }
    #[inline]
    pub fn pieces_cp(&self, c: Color, pt: PieceType) -> Bitboard {
        self.base.pieces_cp(c, pt)
    }
    #[inline]
    pub fn pieces_pp(&self, pt0: PieceType, pt1: PieceType) -> Bitboard {
        self.base.pieces_pp(pt0, pt1)
    }
    #[inline]
    #[allow(dead_code)]
    pub fn pieces_cpp(&self, c: Color, pt0: PieceType, pt1: PieceType) -> Bitboard {
        self.base.pieces_cpp(c, pt0, pt1)
    }
    #[inline]
    #[allow(dead_code)]
    pub fn pieces_ppp(&self, pt0: PieceType, pt1: PieceType, pt2: PieceType) -> Bitboard {
        self.base.pieces_ppp(pt0, pt1, pt2)
    }
    #[inline]
    pub fn pieces_cppp(
        &self,
        c: Color,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
    ) -> Bitboard {
        self.base.pieces_cppp(c, pt0, pt1, pt2)
    }
    #[inline]
    #[allow(dead_code)]
    pub fn pieces_pppp(
        &self,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
    ) -> Bitboard {
        self.base.pieces_pppp(pt0, pt1, pt2, pt3)
    }
    #[inline]
    #[allow(dead_code)]
    pub fn pieces_cpppp(
        &self,
        c: Color,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
    ) -> Bitboard {
        self.base.pieces_cpppp(c, pt0, pt1, pt2, pt3)
    }
    #[inline]
    pub fn pieces_ppppp(
        &self,
        pt0: PieceType,
        pt1: PieceType,
        pt2: PieceType,
        pt3: PieceType,
        pt4: PieceType,
    ) -> Bitboard {
        self.base.pieces_ppppp(pt0, pt1, pt2, pt3, pt4)
    }
    #[inline]
    pub fn pieces_golds(&self) -> Bitboard {
        self.base.pieces_golds()
    }
    #[inline]
    pub fn piece_on(&self, sq: Square) -> Piece {
        self.base.piece_on(sq)
    }
    #[inline]
    pub fn occupied_bb(&self) -> Bitboard {
        self.base.occupied_bb()
    }
    #[inline]
    pub fn empty_bb(&self) -> Bitboard {
        self.base.empty_bb()
    }
    #[inline]
    pub fn hand(&self, c: Color) -> Hand {
        self.base.hand(c)
    }
    #[inline]
    pub fn side_to_move(&self) -> Color {
        self.base.side_to_move()
    }
    #[inline]
    pub fn king_square(&self, c: Color) -> Square {
        self.base.king_square(c)
    }
    #[inline]
    pub fn attackers_to(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        self.base.attackers_to(color_of_attackers, to, occupied)
    }
    #[inline]
    pub fn attackers_to_except_king(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        self.base
            .attackers_to_except_king(color_of_attackers, to, occupied)
    }
    #[inline]
    pub fn attackers_to_except_king_lance_pawn(
        &self,
        color_of_attackers: Color,
        to: Square,
        occupied: &Bitboard,
    ) -> Bitboard {
        self.base
            .attackers_to_except_king_lance_pawn(color_of_attackers, to, occupied)
    }
    #[inline]
    pub fn attackers_to_both_color(&self, to: Square, occupied: &Bitboard) -> Bitboard {
        self.base.attackers_to_both_color(to, occupied)
    }
    #[allow(dead_code)]
    pub fn init_states(&mut self) {
        self.states.truncate(0);
        self.states.push(StateInfo::new());
    }
    pub fn init_states_and_push(&mut self, state: StateInfo) {
        self.states.truncate(0);
        self.states.push(state);
    }
    #[inline]
    fn st(&self) -> &StateInfo {
        self.states.last().unwrap()
    }
    #[inline]
    fn st_mut(&mut self) -> &mut StateInfo {
        self.states.last_mut().unwrap()
    }
    pub fn is_capture_after_move(&self) -> bool {
        self.st().is_capture_move()
    }
    #[allow(dead_code)]
    #[inline]
    pub fn slider_blockers_and_pinners(
        &self,
        sliders: &Bitboard,
        color_of_sliders: Color,
        sq: Square,
    ) -> (Bitboard, Bitboard) {
        self.base
            .slider_blockers_and_pinners(sliders, color_of_sliders, sq)
    }
    pub fn blockers_for_king(&self, color_of_king: Color) -> Bitboard {
        self.st().check_info.blockers_for_king(color_of_king)
    }
    pub fn pinners_for_king(&self, color_of_king: Color) -> Bitboard {
        self.st().check_info.pinners_for_king(color_of_king)
    }
    pub fn pseudo_legal<T: IsSearchingTrait>(&self, m: Move) -> bool {
        let us = self.side_to_move();
        let to;
        if m.is_drop() {
            let pc_dropped = m.piece_dropped();
            if Color::new(pc_dropped) != us {
                return false;
            }
            let pt_dropped = PieceType::new(pc_dropped);
            if !self.hand(us).exist(pt_dropped) {
                return false;
            }
            to = m.to();
            if self.piece_on(to) != Piece::EMPTY {
                return false;
            }
            let checkers = self.checkers();
            match checkers.count_ones() {
                0 => {}
                1 => {
                    let check_sq = checkers.lsb_unchecked();
                    let droppables = Bitboard::between_mask(check_sq, self.king_square(us));
                    if !droppables.is_set(to) {
                        return false;
                    }
                }
                2 => return false,
                _ => unreachable!(),
            }
            if pt_dropped == PieceType::PAWN {
                if (self.pieces_cp(us, PieceType::PAWN) & Bitboard::file_mask(File::new(to)))
                    .to_bool()
                {
                    // two pawns
                    return false;
                }
                let delta = if us == Color::BLACK {
                    Square::DELTA_N
                } else {
                    Square::DELTA_S
                };
                let them = us.inverse();
                if to.add_unchecked(delta) == self.king_square(them)
                    && self.is_drop_pawn_mate(us, to)
                {
                    // drop pawn mate
                    return false;
                }
            }
        } else {
            let from = m.from();
            let pc_from = self.piece_on(from);
            if pc_from == Piece::EMPTY
                || pc_from != m.piece_moved_before_move()
                || Color::new(pc_from) != us
            {
                return false;
            }
            to = m.to();
            if self.pieces_c(us).is_set(to) {
                return false;
            }
            let pt_from = PieceType::new(pc_from);
            if !ATTACK_TABLE
                .attack(pt_from, us, from, &self.occupied_bb())
                .is_set(to)
            {
                return false;
            }

            if m.is_promotion() {
                if !pc_from.is_promotable() {
                    return false;
                }
                if T::IS_SEARCHING {
                    debug_assert!(
                        Rank::new(from).is_opponent_field(us)
                            || Rank::new(to).is_opponent_field(us)
                    );
                } else if !Rank::new(from).is_opponent_field(us)
                    && !Rank::new(to).is_opponent_field(us)
                {
                    return false;
                }
            } else {
                match pt_from {
                    PieceType::PAWN => {
                        if T::IS_SEARCHING {
                            if Rank::new(to).is_opponent_field(us) {
                                // pawn unpromote move
                                return false;
                            }
                        } else if Rank::new(to).is_in_front_of(us, RankAsBlack::RANK2) {
                            return false;
                        }
                    }
                    PieceType::LANCE => {
                        if T::IS_SEARCHING {
                            // Rank1(Rank9): illegal.
                            // Rank2(Rank8): legal but avoid unpromote move.
                            // Rank3(Rank7): legal but avoid unpromoted and uncapture move.
                            let r = Rank::new(to);
                            if r.is_in_front_of(us, RankAsBlack::RANK3)
                                || (r.is_in_front_of(us, RankAsBlack::RANK4)
                                    && !m.is_capture(&self))
                            {
                                return false;
                            }
                        } else {
                            if Rank::new(to).is_in_front_of(us, RankAsBlack::RANK2) {
                                return false;
                            }
                        }
                    }
                    PieceType::KNIGHT => {
                        if Rank::new(to).is_in_front_of(us, RankAsBlack::RANK3) {
                            return false;
                        }
                    }
                    PieceType::BISHOP | PieceType::ROOK => {
                        if T::IS_SEARCHING
                            && (Rank::new(from).is_opponent_field(us)
                                || Rank::new(to).is_opponent_field(us))
                        {
                            // legal but avoid unpromote move.
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            let checkers = self.checkers();
            if checkers.to_bool() {
                if pt_from == PieceType::KING {
                    if self
                        .attackers_to(
                            us.inverse(),
                            to,
                            &(self.occupied_bb() ^ Bitboard::square_mask(from)),
                        )
                        .to_bool()
                    {
                        // not evasion.
                        return false;
                    }
                } else {
                    match checkers.count_ones() {
                        0 => {}
                        1 => {
                            // evasion.
                            let checker_sq = checkers.lsb_unchecked();
                            let movables =
                                Bitboard::between_mask(checker_sq, self.king_square(us)) | checkers;
                            if !movables.is_set(to) {
                                return false;
                            }
                        }
                        2 => return false, // if double check, king must move.
                        _ => unreachable!(),
                    }
                }
            }
        }
        true
    }
    pub fn legal(&self, m: Move) -> bool {
        // Repetition king check is illegal, but this function return legal.
        // Repetition king check is judged illegal(mated) in search functions.
        if m.is_drop() {
            return true;
        }
        let from = m.from();
        let us = self.side_to_move();
        if PieceType::new(self.piece_on(from)) == PieceType::KING {
            let them = us.inverse();
            return !self
                .attackers_to(
                    them,
                    m.to(),
                    &(self.occupied_bb() ^ Bitboard::square_mask(from)),
                )
                .to_bool();
        }
        !self.blockers_for_king(us).is_set(from)
            || is_aligned_and_sq2_is_not_between_sq0_and_sq1(from, m.to(), self.king_square(us))
    }
    fn min_attacker(
        &self,
        to: Square,
        side_to_move_attackers: &Bitboard,
        occupied: &mut Bitboard,
        attackers: &mut Bitboard,
    ) -> PieceType {
        let mut b;
        macro_rules! attacker_found {
            ($pt: expr) => {{
                b = *side_to_move_attackers & self.pieces_p($pt);
                b.to_bool()
            }};
        }
        if !attacker_found!(PieceType::PAWN)
            && !attacker_found!(PieceType::LANCE)
            && !attacker_found!(PieceType::KNIGHT)
            && !attacker_found!(PieceType::PRO_PAWN)
            && !attacker_found!(PieceType::PRO_LANCE)
            && !attacker_found!(PieceType::PRO_KNIGHT)
            && !attacker_found!(PieceType::SILVER)
            && !attacker_found!(PieceType::PRO_SILVER)
            && !attacker_found!(PieceType::GOLD)
            && !attacker_found!(PieceType::BISHOP)
            && !attacker_found!(PieceType::HORSE)
            && !attacker_found!(PieceType::ROOK)
            && !attacker_found!(PieceType::DRAGON)
        {
            return PieceType::KING;
        }
        let sq = b.lsb_unchecked();
        *occupied ^= Bitboard::square_mask(sq);

        // add a piece behind of sq. (and add a piece of sq. but it supporsed to be erased after.)
        match Relation::new(sq, to) {
            Relation::MISC => {}
            Relation::FILE_NS => {
                *attackers |= ATTACK_TABLE.lance.attack(Color::BLACK, to, occupied)
                    & self.pieces_cppp(
                        Color::WHITE,
                        PieceType::ROOK,
                        PieceType::DRAGON,
                        PieceType::LANCE,
                    );
            }
            Relation::FILE_SN => {
                *attackers |= ATTACK_TABLE.lance.attack(Color::WHITE, to, occupied)
                    & self.pieces_cppp(
                        Color::BLACK,
                        PieceType::ROOK,
                        PieceType::DRAGON,
                        PieceType::LANCE,
                    );
            }
            Relation::RANK_EW | Relation::RANK_WE => {
                *attackers |= ATTACK_TABLE.rook.magic(to).attack(occupied)
                    & (self.pieces_pp(PieceType::ROOK, PieceType::DRAGON));
            }
            Relation::DIAG_NESW
            | Relation::DIAG_NWSE
            | Relation::DIAG_SWNE
            | Relation::DIAG_SENW => {
                *attackers |= ATTACK_TABLE.bishop.magic(to).attack(occupied)
                    & self.pieces_pp(PieceType::BISHOP, PieceType::HORSE);
            }
            _ => unreachable!(),
        }
        // erase a piece of sq.
        *attackers &= *occupied;
        PieceType::new(self.piece_on(sq))
    }
    pub fn see_ge(&self, m: Move, threshold: Value) -> bool {
        let to = m.to();
        let mut balance = capture_piece_value(self.piece_on(to)) - threshold;
        if balance < Value::ZERO {
            return false;
        }
        let is_drop = m.is_drop();
        let mut next_victim = if is_drop {
            m.piece_type_dropped()
        } else {
            PieceType::new(self.piece_on(m.from()))
        };
        balance -= capture_piece_type_value(next_victim);
        // in case next_victim == PieceType::KING return here.
        // ( capture_piece_type_value(PieceType::KING) == Value::ZERO )
        // it is ok if this move is legal.
        if balance >= Value::ZERO {
            return true;
        }
        let mut attackers;
        let mut occupied = self.occupied_bb();
        // "m" is capture, "occupied" become
        // In fact, the bit at the position of "to" should be 0,
        // but in case "m" is non-capture, the same result is obtained for bit 0 or 1.
        // Therefore, there is no problem by xoring "occupied" position of "to".
        occupied ^= Bitboard::square_mask(to);
        if !is_drop {
            occupied ^= Bitboard::square_mask(m.from());
        }
        attackers = self.attackers_to_both_color(to, &occupied) & occupied;
        let us = self.side_to_move();
        let mut side_to_move = us.inverse();
        loop {
            let mut side_to_move_attackers = attackers & self.pieces_c(side_to_move);
            if !(self.pinners_for_king(side_to_move.inverse()) & !occupied).to_bool() {
                side_to_move_attackers &= !self.blockers_for_king(side_to_move);
            }
            if !side_to_move_attackers.to_bool() {
                break;
            }
            next_victim =
                self.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
            side_to_move = side_to_move.inverse();
            debug_assert!(balance < Value::ZERO);
            balance = -balance - Value(1) - capture_piece_type_value(next_victim);
            if balance >= Value::ZERO {
                if next_victim == PieceType::KING
                    && (attackers & self.pieces_c(side_to_move)).to_bool()
                {
                    side_to_move = side_to_move.inverse();
                }
                break;
            }
            debug_assert!(next_victim != PieceType::KING);
        }
        us != side_to_move
    }
    pub fn is_drop_pawn_mate(&self, color_of_pawn: Color, sq_of_pawn: Square) -> bool {
        debug_assert_eq!(
            ATTACK_TABLE
                .pawn
                .attack(color_of_pawn, sq_of_pawn)
                .count_ones(),
            1
        );
        debug_assert_eq!(
            ATTACK_TABLE
                .pawn
                .attack(color_of_pawn, sq_of_pawn)
                .lsb_unchecked(),
            self.king_square(color_of_pawn.inverse())
        );

        if !self
            .attackers_to(color_of_pawn, sq_of_pawn, &self.occupied_bb())
            .to_bool()
        {
            return false; // The pawn has no followers. king can capture the pawn.
        }
        let color_of_difence = color_of_pawn.inverse();
        // other piece's capture.
        // king: NG (recapture)
        // pawn: NG (can not capture)
        // lance: NG (can not capture)
        let capture_candidates = self.attackers_to_except_king_lance_pawn(
            color_of_difence,
            sq_of_pawn,
            &self.occupied_bb(),
        );
        let pawn_file = File::new(sq_of_pawn);
        let pinned = self.blockers_for_king(color_of_difence);
        let not_pinned_for_pawn_capture = !pinned | Bitboard::file_mask(pawn_file);
        let can_captures = capture_candidates & not_pinned_for_pawn_capture;
        if can_captures.to_bool() {
            return false;
        }
        // king escapes
        let ksq = self.king_square(color_of_difence);
        let mut king_escape_candidates =
            ATTACK_TABLE.king.attack(ksq) & !self.pieces_c(color_of_difence);
        debug_assert!(king_escape_candidates.is_set(sq_of_pawn));
        king_escape_candidates ^= Bitboard::square_mask(sq_of_pawn); // more faster than Bitboard::clear()
        let occupied_after_drop_pawn = self.occupied_bb() ^ Bitboard::square_mask(sq_of_pawn);
        for to in king_escape_candidates {
            if !self
                .attackers_to(color_of_pawn, to, &occupied_after_drop_pawn)
                .to_bool()
            {
                return false;
            }
        }
        true
    }
    pub fn is_repetition(&self) -> Repetition {
        const MAX_REPETITION_PLY: i32 = 16;
        let end = std::cmp::min(MAX_REPETITION_PLY, self.st().plies_from_null);

        // Repetition state takes at least 4 moves.
        if end < 4 {
            return Repetition::Not;
        }

        let mut state_index = self.states.len() - 3;
        for i in (4..=end).step_by(2) {
            state_index -= 2;
            let st = &self.states[state_index];
            if self.key() == st.key() {
                let us = self.side_to_move();
                if i <= self.st().continuous_check(us) {
                    return Repetition::Lose;
                }
                if i <= self.st().continuous_check(us.inverse()) {
                    return Repetition::Win;
                }
                return Repetition::Draw;
            } else if self.st().board_key == st.board_key {
                if self
                    .st()
                    .hand_of_side_to_move
                    .is_equal_or_superior(st.hand_of_side_to_move)
                {
                    return Repetition::Superior;
                }
                if st
                    .hand_of_side_to_move
                    .is_equal_or_superior(self.st().hand_of_side_to_move)
                {
                    return Repetition::Inferior;
                }
            }
        }
        Repetition::Not
    }
    pub fn is_entering_king_win(&self) -> bool {
        // CSA rule.

        // 一 宣言側の手番である。
        // 六 宣言側の持ち時間が残っている。

        // 五 宣言側の玉に王手がかかっていない。
        if self.in_check() {
            return false;
        }

        // 二 宣言側の玉が敵陣三段目以内に入っている。
        let us = self.side_to_move();
        if !Rank::new(self.king_square(us)).is_opponent_field(us) {
            return false;
        }

        // 四 宣言側の敵陣三段目以内の駒は、玉を除いて10枚以上存在する。
        let own_pieces_count =
            (self.pieces_c(us) & Bitboard::opponent_field_mask(us)).count_ones() - 1;
        if own_pieces_count < 10 {
            return false;
        }

        // 三 宣言側が、大駒5点小駒1点で計算して
        //     先手の場合28点以上の持点がある。
        //     後手の場合27点以上の持点がある。
        //     点数の対象となるのは、宣言側の持駒と敵陣三段目以内に存在する玉を除く宣言側の駒のみである。
        let own_big_pieces_count = (self.pieces_cpppp(
            us,
            PieceType::BISHOP,
            PieceType::ROOK,
            PieceType::HORSE,
            PieceType::DRAGON,
        ) & Bitboard::opponent_field_mask(us))
        .count_ones();
        let own_small_pieces_count = own_pieces_count - own_big_pieces_count;
        let hand = self.hand(us);
        let val = own_small_pieces_count
            + hand.num(PieceType::PAWN)
            + hand.num(PieceType::LANCE)
            + hand.num(PieceType::KNIGHT)
            + hand.num(PieceType::SILVER)
            + hand.num(PieceType::GOLD)
            + (own_big_pieces_count + hand.num(PieceType::BISHOP) + hand.num(PieceType::ROOK)) * 5;
        let thresh = if us == Color::BLACK { 28 } else { 27 };
        if val < thresh {
            return false;
        }
        true
    }
    #[inline]
    pub fn key(&self) -> Key {
        self.st().key()
    }
    #[inline]
    fn board_key(&self) -> Key {
        self.st().board_key
    }
    #[inline]
    fn hand_key(&self) -> Key {
        self.st().hand_key
    }
    #[inline]
    pub fn material(&self) -> Value {
        self.st().material
    }
    pub fn material_diff(&self) -> Value {
        self.st().material - self.states[self.states.len() - 2].material
    }
    pub fn captured_piece(&self) -> Piece {
        self.st().captured_piece
    }
    #[allow(dead_code)]
    #[inline]
    pub fn print(&self) {
        self.base.print();
        println!("key: {}", self.key().0);
    }
    #[inline]
    pub fn to_sfen(&self) -> String {
        self.base.to_sfen()
    }
    #[allow(dead_code)]
    #[inline]
    pub fn to_csa_string(&self) -> String {
        self.base.to_csa_string()
    }
    #[inline]
    pub fn checkers(&self) -> Bitboard {
        self.st().checkers_bb
    }
    #[inline]
    pub fn in_check(&self) -> bool {
        self.checkers().to_bool()
    }
    #[allow(dead_code)]
    pub fn nodes_searched(&self) -> i64 {
        (*self.nodes).load(Ordering::Relaxed)
    }
    pub fn gives_check(&self, m: Move) -> bool {
        let to = m.to();
        if m.is_drop() {
            let pt_to = m.piece_type_dropped();
            if self.st().check_info.check_squares[pt_to.0 as usize].is_set(to) {
                return true;
            }
        } else {
            let from = m.from();
            let pc_from = self.piece_on(from);
            let pc_to = if m.is_promotion() {
                pc_from.to_promote()
            } else {
                pc_from
            };
            let pt_to = PieceType::new(pc_to);
            // direct check
            if self.st().check_info.check_squares[pt_to.0 as usize].is_set(to) {
                return true;
            }
            let us = self.side_to_move();
            let them = us.inverse();
            // discovered check
            if self.st().check_info.blockers_for_king(them).is_set(from)
                && !is_aligned_and_sq2_is_not_between_sq0_and_sq1(from, to, self.king_square(them))
            {
                return true;
            }
        }
        false
    }
    pub fn do_move(&mut self, m: Move, gives_check: bool) {
        debug_assert!(self.is_ok());
        (*self.nodes).fetch_add(1, Ordering::Relaxed);
        let mut board_key = self.board_key() ^ Zobrist::COLOR;
        let mut hand_key = self.hand_key();
        {
            // I want Rust to have something like C++ emplace_back().
            let state = unsafe { StateInfo::new_from_old_state(self.st()) };
            self.states.push(state);
        }
        self.base.game_ply += 1;
        self.st_mut().plies_from_null += 1;

        let us = self.side_to_move();
        let them = us.inverse();
        let to = m.to();
        let captured_piece;
        if m.is_drop() {
            let pc_to = m.piece_dropped();
            let pt_to = PieceType::new(pc_to);
            let hand_num = self.hand(us).num(pt_to);
            let old_eval_index = EvalIndex(EvalIndex::new_hand(pc_to).0 + hand_num as usize);
            let new_eval_index = EvalIndex(EvalIndex::new_board(pc_to).0 + to.0 as usize);
            self.st_mut().changed_eval_index.old_index = old_eval_index;
            self.st_mut().changed_eval_index.new_index = new_eval_index;
            let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
            self.eval_index_to_eval_list_index
                .set(new_eval_index, eval_list_index);
            self.eval_list
                .set(eval_list_index, Color::BLACK, new_eval_index);
            self.eval_list
                .set(eval_list_index, Color::WHITE, new_eval_index.inverse());
            hand_key ^= Zobrist::get_hand(pt_to, hand_num, us);
            board_key ^= Zobrist::get_field(pt_to, to, us);
            self.base.hands[us.0 as usize].minus_one(pt_to);
            self.base.put_piece(pc_to, to);

            // set golds_bb before using attackers_to_except_king.
            self.base.set_golds_bb();
            if gives_check {
                // only one direct check.
                self.st_mut().checkers_bb = Bitboard::square_mask(to);
                self.st_mut().continuous_checks[us.0 as usize] += 2;
            } else {
                self.st_mut().checkers_bb = Bitboard::ZERO;
                self.st_mut().continuous_checks[us.0 as usize] = 0;
            }
            captured_piece = Piece::EMPTY;
        } else {
            let from = m.from();
            let pc_from = self.piece_on(from);
            let pt_from = PieceType::new(pc_from);

            self.base.remove_piece(pc_from, from);
            if m.is_capture(&self) {
                captured_piece = self.piece_on(to);
                let pt_captured = PieceType::new(captured_piece);
                self.base.xor_bbs(them, pt_captured, to);
                let pt_captured_demoted = pt_captured.to_demote_if_possible();
                self.base.hands[us.0 as usize].plus_one(pt_captured_demoted);
                let hand_num = self.hand(us).num(pt_captured_demoted);

                let old_eval_index =
                    EvalIndex(EvalIndex::new_board(captured_piece).0 + to.0 as usize);
                let new_eval_index = EvalIndex(
                    EvalIndex::new_hand(Piece::new(us, pt_captured_demoted)).0 + hand_num as usize,
                );
                self.st_mut().changed_eval_index_captured.old_index = old_eval_index;
                self.st_mut().changed_eval_index_captured.new_index = new_eval_index;
                let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
                self.eval_index_to_eval_list_index
                    .set(new_eval_index, eval_list_index);
                self.eval_list
                    .set(eval_list_index, Color::BLACK, new_eval_index);
                self.eval_list
                    .set(eval_list_index, Color::WHITE, new_eval_index.inverse());

                board_key ^= Zobrist::get_field(pt_captured, to, them);
                hand_key ^= Zobrist::get_hand(pt_captured_demoted, hand_num, us);
                self.st_mut().material += if us == Color::BLACK {
                    capture_piece_type_value(pt_captured)
                } else {
                    -capture_piece_type_value(pt_captured)
                };
            } else {
                captured_piece = Piece::EMPTY;
            }
            let pc_to = if m.is_promotion() {
                self.st_mut().material += if us == Color::BLACK {
                    promote_piece_type_value(pt_from)
                } else {
                    -promote_piece_type_value(pt_from)
                };
                pc_from.to_promote()
            } else {
                pc_from
            };
            self.base.put_piece(pc_to, to);
            let pt_to = PieceType::new(pc_to);
            if pt_to == PieceType::KING {
                // If moved piece is King, changed_eval_index is not used.
                //self.st_mut().changed_eval_index.old_index = EvalIndex(0);
                //self.st_mut().changed_eval_index.new_index = EvalIndex(0);
                self.base.king_squares[us.0 as usize] =
                    self.pieces_cp(us, PieceType::KING).lsb_unchecked();
            } else {
                let old_eval_index = EvalIndex(EvalIndex::new_board(pc_from).0 + from.0 as usize);
                let new_eval_index = EvalIndex(EvalIndex::new_board(pc_to).0 + to.0 as usize);
                self.st_mut().changed_eval_index.old_index = old_eval_index;
                self.st_mut().changed_eval_index.new_index = new_eval_index;
                let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
                self.eval_index_to_eval_list_index
                    .set(new_eval_index, eval_list_index);
                self.eval_list
                    .set(eval_list_index, Color::BLACK, new_eval_index);
                self.eval_list
                    .set(eval_list_index, Color::WHITE, new_eval_index.inverse());
            }

            board_key ^= Zobrist::get_field(pt_from, from, us);
            board_key ^= Zobrist::get_field(pt_to, to, us);

            // set golds_bb before using attackers_to_except_king.
            self.base.set_golds_bb();

            if gives_check {
                self.st_mut().checkers_bb =
                    self.attackers_to_except_king(us, self.king_square(them), &self.occupied_bb())
                        & self.pieces_c(us);
                self.st_mut().continuous_checks[us.0 as usize] += 2;
            } else {
                self.st_mut().checkers_bb = Bitboard::ZERO;
                self.st_mut().continuous_checks[us.0 as usize] = 0;
            };
        }
        self.base.side_to_move = them;
        self.st_mut().board_key = board_key;
        self.st_mut().hand_key = hand_key;
        self.st_mut().hand_of_side_to_move = self.hand(them);
        self.st_mut().captured_piece = captured_piece;
        self.st_mut().check_info = CheckInfo::new(&self.base);
        debug_assert!(self.is_ok());
    }
    pub fn undo_move(&mut self, m: Move) {
        debug_assert!(self.is_ok());
        let us = self.side_to_move();
        let them = us.inverse();
        let to = m.to();
        if m.is_drop() {
            let pc_dropped = m.piece_dropped();
            let pt_dropped = PieceType::new(pc_dropped);
            self.base.remove_piece(pc_dropped, to);
            self.base.hands[them.0 as usize].plus_one(pt_dropped);

            let hand_num = self.hand(them).num(pt_dropped);
            let old_eval_index = EvalIndex(EvalIndex::new_board(pc_dropped).0 + to.0 as usize);
            let new_eval_index = EvalIndex(EvalIndex::new_hand(pc_dropped).0 + hand_num as usize);
            let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
            self.eval_index_to_eval_list_index
                .set(new_eval_index, eval_list_index);
            self.eval_list
                .set(eval_list_index, Color::BLACK, new_eval_index);
            self.eval_list
                .set(eval_list_index, Color::WHITE, new_eval_index.inverse());
        } else {
            let pc_to = self.piece_on(to);
            if self.st().is_capture_move() {
                let pc_captured = self.st().captured_piece;
                let pt_captured = PieceType::new(pc_captured);
                let pt_captured_demoted = pt_captured.to_demote_if_possible();

                let hand_num = self.hand(them).num(pt_captured_demoted);
                let old_eval_index = EvalIndex(
                    EvalIndex::new_hand(Piece::new(them, pt_captured_demoted)).0
                        + hand_num as usize,
                );
                let new_eval_index = EvalIndex(EvalIndex::new_board(pc_captured).0 + to.0 as usize);
                let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
                self.eval_index_to_eval_list_index
                    .set(new_eval_index, eval_list_index);
                self.eval_list
                    .set(eval_list_index, Color::BLACK, new_eval_index);
                self.eval_list
                    .set(eval_list_index, Color::WHITE, new_eval_index.inverse());

                self.base.exchange_pieces(pc_captured, to);
                self.base.hands[them.0 as usize].minus_one(pt_captured_demoted);
            } else {
                self.base.remove_piece(pc_to, to);
            }
            let pc_from = if m.is_promotion() {
                pc_to.to_demote()
            } else {
                pc_to
            };
            let from = m.from();
            self.base.put_piece(pc_from, from);
            if pc_to.is_king() {
                self.base.king_squares[them.0 as usize] = from;
            } else {
                let old_eval_index = EvalIndex(EvalIndex::new_board(pc_to).0 + to.0 as usize);
                let new_eval_index = EvalIndex(EvalIndex::new_board(pc_from).0 + from.0 as usize);
                self.st_mut().changed_eval_index.old_index = old_eval_index;
                self.st_mut().changed_eval_index.new_index = new_eval_index;
                let eval_list_index = self.eval_index_to_eval_list_index.get(old_eval_index);
                self.eval_index_to_eval_list_index
                    .set(new_eval_index, eval_list_index);
                self.eval_list
                    .set(eval_list_index, Color::BLACK, new_eval_index);
                self.eval_list
                    .set(eval_list_index, Color::WHITE, new_eval_index.inverse());
            }
        }
        self.base.set_golds_bb();
        self.base.side_to_move = them;
        self.base.game_ply -= 1;
        self.states.pop();
        debug_assert!(self.is_ok());
    }
    pub fn do_null_move(&mut self) {
        debug_assert!(self.is_ok());
        {
            let state = self.st().clone();
            self.states.push(state);
        }
        let them = self.side_to_move().inverse();
        self.base.side_to_move = them;
        self.st_mut().plies_from_null = 0;
        self.st_mut().continuous_checks = [0, 0];
        self.st_mut().board_key ^= Zobrist::COLOR;
        self.st_mut().hand_of_side_to_move = self.hand(them);
        self.st_mut().captured_piece = Piece::EMPTY;
        self.st_mut().check_info = CheckInfo::new(&self.base);
        debug_assert!(self.is_ok());
    }
    pub fn undo_null_move(&mut self) {
        debug_assert!(!self.checkers().to_bool());
        self.states.pop();
        self.base.side_to_move = self.side_to_move().inverse();
    }
    pub fn reserve_states(&mut self) {
        self.states
            .reserve(self.base.game_ply as usize + MAX_PLY as usize);
    }
    pub fn effect_bb_of_checker_where_king_cannot_escape(
        &self,
        checker_sq: Square,
        checker_pc: Piece,
        occupied: &Bitboard,
    ) -> Bitboard {
        let checker_pt = PieceType::new(checker_pc);
        let checker_color = Color::new(checker_pc);
        match checker_pt {
            PieceType::PAWN | PieceType::KNIGHT => Bitboard::ZERO,
            PieceType::LANCE => ATTACK_TABLE.lance.pseudo_attack(checker_color, checker_sq),
            PieceType::SILVER => ATTACK_TABLE.silver.attack(checker_color, checker_sq),
            PieceType::GOLD
            | PieceType::PRO_PAWN
            | PieceType::PRO_LANCE
            | PieceType::PRO_KNIGHT
            | PieceType::PRO_SILVER => ATTACK_TABLE.gold.attack(checker_color, checker_sq),
            PieceType::BISHOP => ATTACK_TABLE.bishop.magic(checker_sq).pseudo_attack(),
            PieceType::HORSE => {
                ATTACK_TABLE.bishop.magic(checker_sq).pseudo_attack()
                    | ATTACK_TABLE.king.attack(checker_sq)
            }
            PieceType::ROOK => ATTACK_TABLE.rook.magic(checker_sq).pseudo_attack(),
            PieceType::DRAGON => {
                let opp_king_color = checker_color.inverse();
                let opp_king_sq = self.king_square(opp_king_color);
                if Relation::new(opp_king_sq, checker_sq).is_diag() {
                    ATTACK_TABLE.rook.magic(checker_sq).attack(occupied)
                        | ATTACK_TABLE.king.attack(checker_sq)
                } else {
                    ATTACK_TABLE.rook.magic(checker_sq).pseudo_attack()
                        | ATTACK_TABLE.king.attack(checker_sq)
                }
            }
            _ => unreachable!(),
        }
    }
    fn mate_drop_move_in_1ply<PTT: PieceTypeTrait>(&self, us: Color) -> Option<Move> {
        let them = us.inverse();
        let ksq = self.king_square(them);
        let target = self.empty_bb();
        // king neighbor
        let to_bb = target & ATTACK_TABLE.attack(PTT::PIECE_TYPE, them, ksq, &Bitboard::ALL);
        fn bb_of_king_cannot_escape(
            dropped_piece_type: PieceType,
            dropped_color: Color,
            dropped_sq: Square,
        ) -> Bitboard {
            match dropped_piece_type {
                PieceType::LANCE => ATTACK_TABLE.lance.pseudo_attack(dropped_color, dropped_sq),
                PieceType::KNIGHT => Bitboard::ZERO,
                PieceType::SILVER => ATTACK_TABLE.silver.attack(dropped_color, dropped_sq),
                PieceType::GOLD => ATTACK_TABLE.gold.attack(dropped_color, dropped_sq),
                PieceType::BISHOP => ATTACK_TABLE.bishop.magic(dropped_sq).pseudo_attack(),
                PieceType::ROOK => ATTACK_TABLE.rook.magic(dropped_sq).pseudo_attack(),
                _ => unreachable!(),
            }
        }
        'outer: for to in to_bb {
            if PTT::PIECE_TYPE != PieceType::KNIGHT // Knight needs no support.
                && !self.attackers_to(us, to, &self.occupied_bb()).to_bool()
            {
                // support not exist.
                continue;
            }
            let king_escape_candidates = ATTACK_TABLE.king.attack(ksq)
                & !self.pieces_c(them)
                & !bb_of_king_cannot_escape(PTT::PIECE_TYPE, us, to);
            for escape_sq in king_escape_candidates {
                let tmp_occupied = self.occupied_bb() ^ Bitboard::square_mask(to);
                if !self.attackers_to(us, escape_sq, &tmp_occupied).to_bool() {
                    continue 'outer;
                }
            }

            let capture_candidates = self.attackers_to_except_king(them, to, &self.occupied_bb());
            for capture_candidate_sq in capture_candidates {
                if !self.blockers_for_king(them).is_set(capture_candidate_sq)
                    || is_aligned_and_sq2_is_not_between_sq0_and_sq1(capture_candidate_sq, to, ksq)
                {
                    continue 'outer;
                }
            }
            let pc = Piece::new(us, PTT::PIECE_TYPE);
            let m = Move::new_drop(pc, to);
            return Some(m);
        }
        None
    }
    fn mate_non_drop_move_in_1ply<IsKnight: Bool>(&self, us: Color) -> Option<Move> {
        let them = us.inverse();
        let ksq = self.king_square(them);
        if IsKnight::BOOL {
            let pc = Piece::new(us, PieceType::KNIGHT);
            let from_bb =
                self.pieces_cp(us, PieceType::KNIGHT) & Bitboard::proximity_check_mask(pc, ksq);
            for from in from_bb {
                if self.blockers_for_king(us).is_set(from) {
                    continue;
                }
                let to_bb = ATTACK_TABLE.knight.attack(us, from) & !self.pieces_c(us);
                'to_loop: for to in to_bb {
                    if !ATTACK_TABLE.knight.attack(us, to).is_set(ksq) {
                        continue;
                    }
                    let mut pos_base = self.base.clone();
                    pos_base.remove_piece(pc, from);
                    if pos_base.piece_on(to) == Piece::EMPTY {
                        pos_base.put_piece(pc, to);
                    } else {
                        pos_base.exchange_pieces(pc, to);
                    }
                    pos_base.set_golds_bb();
                    let (blockers, _pinners) =
                        pos_base.slider_blockers_and_pinners(&pos_base.pieces_c(us), us, ksq);
                    let king_escape_candidates =
                        ATTACK_TABLE.king.attack(ksq) & !pos_base.pieces_c(them);
                    for escape_sq in king_escape_candidates {
                        if !pos_base
                            .attackers_to(
                                us,
                                escape_sq,
                                &(pos_base.occupied_bb() ^ Bitboard::square_mask(ksq)),
                            )
                            .to_bool()
                        {
                            continue 'to_loop;
                        }
                    }
                    if self.blockers_for_king(them).is_set(from)
                        && !is_aligned_and_sq2_is_not_between_sq0_and_sq1(from, to, ksq)
                    {
                        // num of checkers is 2 => can't capture
                    } else {
                        let capture_candidates =
                            pos_base.attackers_to_except_king(them, to, &pos_base.occupied_bb());
                        for capture_sq in capture_candidates {
                            if !blockers.is_set(capture_sq)
                                || is_aligned_and_sq2_is_not_between_sq0_and_sq1(
                                    capture_sq, to, ksq,
                                )
                            {
                                continue 'to_loop;
                            }
                        }
                    }
                    return Some(Move::new_unpromote(from, to, pc));
                }
            }
        } else {
            let to_bb = ATTACK_TABLE.king.attack(ksq) & !self.pieces_c(us);
            for to in to_bb {
                // from_bb includes knight. But unpromoted knight's move can't checkmate.
                let from_bb = self.attackers_to_except_king(us, to, &self.occupied_bb());
                for from in from_bb {
                    let mut pos_base = self.base.clone();
                    let pc = self.piece_on(from);
                    if !Bitboard::proximity_check_mask(pc, ksq).is_set(from) {
                        continue;
                    }
                    pos_base.remove_piece(pc, from);
                    if pos_base.piece_on(to) == Piece::EMPTY {
                        pos_base.put_piece(pc, to);
                    } else {
                        pos_base.exchange_pieces(pc, to);
                    }
                    pos_base.set_golds_bb();
                    let (blockers, _pinners) =
                        pos_base.slider_blockers_and_pinners(&pos_base.pieces_c(us), us, ksq);
                    let pt = PieceType::new(pc);
                    let rank_from = Rank::new(from);
                    let rank_to = Rank::new(to);
                    let tmp_occupied = pos_base.occupied_bb();
                    if !pos_base.attackers_to(us, to, &tmp_occupied).to_bool() {
                        // King can capture this.
                        continue;
                    }
                    if pt.is_promotable()
                        && (rank_from.is_opponent_field(us) || rank_to.is_opponent_field(us))
                    {
                        let attack = ATTACK_TABLE.attack(pt.to_promote(), us, to, &tmp_occupied);
                        if attack.is_set(ksq) {
                            pos_base.remove_piece(pc, to);
                            pos_base.put_piece(pc.to_promote(), to);
                            pos_base.set_golds_bb();
                            let mut king_escape_candidates =
                                ATTACK_TABLE.king.attack(ksq) & !pos_base.pieces_c(them);
                            king_escape_candidates &= !self
                                .effect_bb_of_checker_where_king_cannot_escape(
                                    to,
                                    pc.to_promote(),
                                    &tmp_occupied,
                                );
                            let mut can_escape = false;
                            for escape_sq in king_escape_candidates {
                                if !pos_base
                                    .attackers_to(
                                        us,
                                        escape_sq,
                                        &(tmp_occupied ^ Bitboard::square_mask(ksq)),
                                    )
                                    .to_bool()
                                {
                                    can_escape = true;
                                    break;
                                }
                            }
                            if !can_escape {
                                let can_capture = if self.blockers_for_king(them).is_set(from)
                                    && !is_aligned_and_sq2_is_not_between_sq0_and_sq1(from, to, ksq)
                                {
                                    // num of checkers is 2 => can't capture
                                    false
                                } else {
                                    let mut can_capture = false;
                                    let capture_candidates =
                                        pos_base.attackers_to_except_king(them, to, &tmp_occupied);
                                    for capture_sq in capture_candidates {
                                        if !blockers.is_set(capture_sq)
                                            || is_aligned_and_sq2_is_not_between_sq0_and_sq1(
                                                capture_sq, to, ksq,
                                            )
                                        {
                                            can_capture = true;
                                            break;
                                        }
                                    }
                                    can_capture
                                };
                                if !can_capture
                                    && (!self.blockers_for_king(us).is_set(from)
                                        || is_aligned_and_sq2_is_not_between_sq0_and_sq1(
                                            from,
                                            to,
                                            self.king_square(us),
                                        ))
                                {
                                    return Some(Move::new_promote(from, to, pc));
                                }
                            }
                            pos_base.remove_piece(pc.to_promote(), to);
                            pos_base.put_piece(pc, to);
                            pos_base.set_golds_bb();
                        }
                    }
                    // not promote
                    {
                        let attack = ATTACK_TABLE.attack(pt, us, to, &tmp_occupied);
                        if attack.is_set(ksq) {
                            let mut king_escape_candidates =
                                ATTACK_TABLE.king.attack(ksq) & !pos_base.pieces_c(them);
                            king_escape_candidates &= !self
                                .effect_bb_of_checker_where_king_cannot_escape(
                                    to,
                                    pc,
                                    &tmp_occupied,
                                );
                            let mut can_escape = false;
                            for escape_sq in king_escape_candidates {
                                if !pos_base
                                    .attackers_to(
                                        us,
                                        escape_sq,
                                        &(tmp_occupied ^ Bitboard::square_mask(ksq)),
                                    )
                                    .to_bool()
                                {
                                    can_escape = true;
                                    break;
                                }
                            }
                            if !can_escape {
                                let can_capture = if self.blockers_for_king(them).is_set(from)
                                    && !is_aligned_and_sq2_is_not_between_sq0_and_sq1(from, to, ksq)
                                {
                                    // num of checkers is 2 => can't capture
                                    false
                                } else {
                                    let mut can_capture = false;
                                    let capture_candidates =
                                        pos_base.attackers_to_except_king(them, to, &tmp_occupied);
                                    for capture_sq in capture_candidates {
                                        if !blockers.is_set(capture_sq)
                                            || is_aligned_and_sq2_is_not_between_sq0_and_sq1(
                                                capture_sq, to, ksq,
                                            )
                                        {
                                            can_capture = true;
                                            break;
                                        }
                                    }
                                    can_capture
                                };
                                if !can_capture
                                    && (!self.blockers_for_king(us).is_set(from)
                                        || is_aligned_and_sq2_is_not_between_sq0_and_sq1(
                                            from,
                                            to,
                                            self.king_square(us),
                                        ))
                                {
                                    return Some(Move::new_unpromote(from, to, pc));
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
    pub fn mate_move_in_1ply(&self) -> Option<Move> {
        let us = self.side_to_move();
        let hand = self.hand(us);
        if hand.exist(PieceType::GOLD) {
            if let Some(m) = self.mate_drop_move_in_1ply::<GoldType>(us) {
                return Some(m);
            }
        }
        if hand.exist(PieceType::ROOK) {
            if let Some(m) = self.mate_drop_move_in_1ply::<RookType>(us) {
                return Some(m);
            }
        } else if hand.exist(PieceType::LANCE) {
            if let Some(m) = self.mate_drop_move_in_1ply::<LanceType>(us) {
                return Some(m);
            }
        }
        if hand.exist(PieceType::BISHOP) {
            if let Some(m) = self.mate_drop_move_in_1ply::<BishopType>(us) {
                return Some(m);
            }
        }
        if hand.exist(PieceType::SILVER) {
            if let Some(m) = self.mate_drop_move_in_1ply::<SilverType>(us) {
                return Some(m);
            }
        }
        if hand.exist(PieceType::KNIGHT) {
            if let Some(m) = self.mate_drop_move_in_1ply::<KnightType>(us) {
                return Some(m);
            }
        }
        if let Some(m) = self.mate_non_drop_move_in_1ply::<False>(us) {
            return Some(m);
        }
        if let Some(m) = self.mate_non_drop_move_in_1ply::<True>(us) {
            return Some(m);
        }
        None
    }
    #[allow(dead_code)]
    fn is_ok(&self) -> bool {
        if (self.pieces_c(Color::BLACK) & self.pieces_c(Color::WHITE)).to_bool() {
            panic!("position is ng, line: {}", line!());
        }
        if (self.pieces_c(Color::BLACK) | self.pieces_c(Color::WHITE)) != self.occupied_bb() {
            panic!("position is ng. line: {}", line!());
        }
        if self.pieces_p(PieceType::PAWN)
            ^ self.pieces_p(PieceType::LANCE)
            ^ self.pieces_p(PieceType::KNIGHT)
            ^ self.pieces_p(PieceType::SILVER)
            ^ self.pieces_p(PieceType::BISHOP)
            ^ self.pieces_p(PieceType::ROOK)
            ^ self.pieces_p(PieceType::GOLD)
            ^ self.pieces_p(PieceType::KING)
            ^ self.pieces_p(PieceType::PRO_PAWN)
            ^ self.pieces_p(PieceType::PRO_LANCE)
            ^ self.pieces_p(PieceType::PRO_KNIGHT)
            ^ self.pieces_p(PieceType::PRO_SILVER)
            ^ self.pieces_p(PieceType::HORSE)
            ^ self.pieces_p(PieceType::DRAGON)
            != self.occupied_bb()
        {
            panic!("position is ng. line: {}", line!());
        }
        for i in PieceType::PAWN.0 as usize..PieceType::NUM {
            let pt0 = PieceType(i as i32);
            for j in i + 1..PieceType::NUM {
                let pt1 = PieceType(j as i32);
                if (self.pieces_p(pt0) & self.pieces_p(pt1)).to_bool() {
                    panic!("position is ng. line: {}", line!());
                }
            }
        }
        for &sq in Square::ALL.iter() {
            let pc = self.piece_on(sq);
            if pc == Piece::EMPTY {
                if !self.empty_bb().is_set(sq) {
                    panic!("position is ng. line: {}", line!());
                }
            } else if !self
                .pieces_cp(Color::new(pc), PieceType::new(pc))
                .is_set(sq)
            {
                panic!("position is ng. line: {}", line!());
            }
        }
        for &c in Color::ALL.iter() {
            if self.king_square(c) != self.pieces_cp(c, PieceType::KING).lsb_unchecked() {
                panic!("position is ng. line: {}", line!());
            }
        }
        if self.pieces_ppppp(
            PieceType::GOLD,
            PieceType::PRO_PAWN,
            PieceType::PRO_LANCE,
            PieceType::PRO_KNIGHT,
            PieceType::PRO_SILVER,
        ) != self.base.golds_bb
        {
            panic!("position is ng. line: {}", line!());
        }

        if self.pieces_p(PieceType::KING).count_ones() != 2 {
            panic!("position is ng. line: {}", line!());
        }
        if self.pieces_cp(Color::BLACK, PieceType::KING).count_ones() != 1 {
            panic!("position is ng. line: {}", line!());
        }
        if self.pieces_cp(Color::WHITE, PieceType::KING).count_ones() != 1 {
            panic!("position is ng. line: {}", line!());
        }
        if Square::ALL
            .iter()
            .filter(|&sq| self.piece_on(*sq) == Piece::B_KING)
            .count()
            != 1
        {
            panic!("position is ng. line: {}", line!());
        }
        if Square::ALL
            .iter()
            .filter(|&sq| self.piece_on(*sq) == Piece::W_KING)
            .count()
            != 1
        {
            panic!("position is ng. line: {}", line!());
        }

        {
            let us = self.side_to_move();
            let them = us.inverse();
            let attackers_to_king =
                self.attackers_to(us, self.king_square(them), &self.occupied_bb());
            if attackers_to_king.to_bool() {
                panic!("position is ng. line: {}", line!());
            }
        }

        if 2 < self.checkers().count_ones() {
            panic!("position is ng. line: {}", line!());
        }

        let tmp_state = StateInfo::new_from_position(&self.base);
        if self.material() != tmp_state.material {
            panic!("position is ng. line: {}", line!());
        }

        if self.key() != tmp_state.key() {
            panic!("position is ng. line: {}", line!());
        }

        let mut eval_list_vec_correct = EvalList::new(&self.base)
            .0
            .iter()
            .map(|x| x.iter().map(|y| y.0).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        eval_list_vec_correct.sort();
        let mut eval_list_vec = self
            .eval_list()
            .0
            .iter()
            .map(|x| x.iter().map(|y| y.0).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        eval_list_vec.sort();
        if eval_list_vec != eval_list_vec_correct {
            panic!("position is ng. line: {}", line!());
        }
        true
    }
    pub fn ply(&self) -> i32 {
        self.base.game_ply
    }
    pub fn eval_list(&self) -> &EvalList {
        &self.eval_list
    }
    pub fn eval_list_mut(&mut self) -> &mut EvalList {
        &mut self.eval_list
    }
    pub fn changed_eval_index(&self) -> ChangedEvalIndex {
        self.st().changed_eval_index.clone()
    }
    pub fn changed_eval_index_captured(&self) -> ChangedEvalIndex {
        self.st().changed_eval_index_captured.clone()
    }
    pub fn eval_list_index(&self, eval_index: EvalIndex) -> usize {
        self.eval_index_to_eval_list_index.get(eval_index)
    }
}

#[test]
fn test_position_set() {
    let sfens = [
        "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1",
        "l6nl/5+P1gk/2np1S3/p1p4Pp/3P2Sp1/1PPb2P1P/P5GS1/R8/LN4bKL w RGgsn5p 1",
        "l4S2l/4g1gs1/5p1p1/pr2N1pkp/4Gn3/PP3PPPP/2GPP4/1K7/L3r+s2L w BS2N5Pb 20",
        "6n1l/2+S1k4/2lp4p/1np1B2b1/3PP4/1N1S3rP/1P2+pPP+p1/1p1G5/3KG2r1 b GSN2L4Pgs2p 399",
    ];
    for sfen in sfens.iter() {
        match Position::new_from_sfen(sfen) {
            Ok(pos) => assert_eq!(pos.to_sfen(), sfen.to_string()),
            Err(_) => assert_eq!("".to_string(), sfen.to_string()),
        }
    }

    let sfens = [
        (
            "l6nl/5+P1gk/2np1S3/p1p4Pp/3P2Sp1/1PPb2P1P/P5GS1/R8/LN4bKL w RRGgsn5p 1",
            PieceType::ROOK,
        ),
        (
            "l4S2l/4g1gs1/5p1p1/pr2N1pkp/4Gn3/PP3PPPP/2GPP4/1K7/L3r+s2L w BS2S2N5Pb 20",
            PieceType::SILVER,
        ),
        (
            "6n1l/2+S1k4/2lp4p/1np1B2b1/3PP4/1N1S3rP/1P2+pPP+p1/1p1G5/3KG2r1 b GSN2L4Pgss2p 399",
            PieceType::SILVER,
        ),
    ];
    for &(sfen, pt_twice) in sfens.iter() {
        match Position::new_from_sfen(sfen) {
            Ok(_) => assert_eq!("".to_string(), sfen.to_string()),
            Err(err) => match err {
                SfenError::SameHandPieceTwice { pt } => {
                    assert_eq!(pt, pt_twice);
                }
                _ => assert!(false),
            },
        }
    }

    let sfens = [
        (
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSG1GSNL b - 1",
            Color::BLACK,
        ),
        (
            "lnsg1gsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1",
            Color::WHITE,
        ),
    ];
    for &(sfen, color_of_king_nothing) in sfens.iter() {
        match Position::new_from_sfen(sfen) {
            Ok(_) => assert_eq!("".to_string(), sfen.to_string()),
            Err(err) => match err {
                SfenError::KingIsNothing { c } => {
                    assert_eq!(c, color_of_king_nothing);
                }
                _ => assert!(false),
            },
        }
    }
}

#[test]
fn test_position_attackers_to() {
    let sfens = ["lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1"];
    for sfen in sfens.iter() {
        match Position::new_from_sfen(sfen) {
            Ok(pos) => {
                assert_eq!(pos.to_sfen(), sfen.to_string());
                let attackers = pos.attackers_to(Color::WHITE, Square::SQ52, &pos.occupied_bb());
                assert_eq!(attackers.count_ones(), 4);
                assert!(attackers.is_set(Square::SQ41));
                assert!(attackers.is_set(Square::SQ51));
                assert!(attackers.is_set(Square::SQ61));
                assert!(attackers.is_set(Square::SQ82));
            }
            Err(_) => assert_eq!("".to_string(), sfen.to_string()),
        }
    }
    let sfen = "k8/5+R3/3b1l3/4s4/5pg1+r/4GP3/5LN2/9/K4L3 b - 1";
    match Position::new_from_sfen(sfen) {
        Ok(pos) => {
            let to = Square::SQ45;
            let attackers = pos.attackers_to_both_color(to, &pos.occupied_bb());
            assert_eq!(attackers.count_ones(), 6);
            assert!(attackers.is_set(Square::SQ35));
            assert!(attackers.is_set(Square::SQ37));
            assert!(attackers.is_set(Square::SQ43));
            assert!(attackers.is_set(Square::SQ46));
            assert!(attackers.is_set(Square::SQ54));
            assert!(attackers.is_set(Square::SQ56));
        }
        Err(_) => assert_eq!("".to_string(), sfen.to_string()),
    }
}

#[test]
fn test_position_slider_blockers() {
    let sfen = "4k4/4l4/4P4/9/4K4/9/9/9/9 b - 1";
    match Position::new_from_sfen(sfen) {
        Ok(pos) => {
            assert_eq!(pos.to_sfen(), sfen.to_string());
            let blockers_and_pinners_for_king = pos.slider_blockers_and_pinners(
                &pos.pieces_c(Color::WHITE),
                Color::WHITE,
                pos.king_square(Color::BLACK),
            );
            assert_eq!(
                blockers_and_pinners_for_king.0,
                Bitboard::square_mask(Square::SQ53)
            );
            assert_eq!(
                blockers_and_pinners_for_king.1,
                Bitboard::square_mask(Square::SQ52)
            );
        }
        Err(_) => assert_eq!("".to_string(), sfen.to_string()),
    }
}

#[test]
fn test_state_info() {
    let sfen = "4k4/4l4/4L4/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ53)
    );
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ53)
    );

    let sfen = "4k4/4r4/4R4/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ53)
    );
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ53)
    );

    let sfen = "4k4/4+r4/4+R4/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ53)
    );
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ52)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ53)
    );

    let sfen = "k8/1b7/2B6/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ73)
    );
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ82)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ82)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ73)
    );

    let sfen = "k8/1+b7/2+B6/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ73)
    );
    assert_eq!(
        pos.st().ci().blockers_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ82)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::BLACK),
        Bitboard::square_mask(Square::SQ82)
    );
    assert_eq!(
        pos.st().ci().pinners_for_king(Color::WHITE),
        Bitboard::square_mask(Square::SQ73)
    );
}

#[test]
fn test_position_min_attacker() {
    let sfen = "k8/9/3b1l3/4s4/5pg1+r/4GP3/5RN2/5L3/K4L3 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    let to = Square::SQ45;
    let us = pos.side_to_move();
    let mut occupied = pos.occupied_bb();
    let mut attackers = pos.attackers_to_both_color(to, &occupied);
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::PAWN);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::LANCE);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::KNIGHT);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::SILVER);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::GOLD);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::GOLD);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::ROOK);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::BISHOP);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::LANCE);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::DRAGON);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::LANCE);
    let us = us.inverse();
    let side_to_move_attackers = attackers & pos.pieces_c(us);
    let pt_attacker = pos.min_attacker(to, &side_to_move_attackers, &mut occupied, &mut attackers);
    assert_eq!(pt_attacker, PieceType::KING);
}

#[test]
fn test_position_see_ge() {
    let sfen = "k8/5+R3/3b1l3/4s4/6g1+r/4GP3/5LN2/9/K4L3 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    let to = Square::SQ45;
    let m = Move::new_unpromote(Square::SQ46, to, Piece::B_PAWN);
    assert_eq!(pos.see_ge(m, Value(0)), true);
}

#[test]
fn test_position_gives_check() {
    const CHECK: bool = true;
    const NOT_CHECK: bool = false;
    let array = [
        (
            "8k/9/9/9/9/9/9/9/K8 b Rr 1",
            vec![("R*1b", CHECK), ("R*1h", CHECK), ("R*2b", NOT_CHECK)],
        ),
        (
            "8k/9/9/9/9/9/9/9/K8 w Rr 1",
            vec![("R*9h", CHECK), ("R*9b", CHECK), ("R*8h", NOT_CHECK)],
        ),
        (
            "8k/9/9/9/9/9/9/8G/K7L b Rr 1",
            vec![("1h2h", CHECK), ("1h1g", NOT_CHECK)],
        ),
    ];
    for (sfen, move_candidates) in array.iter() {
        let pos = Position::new_from_sfen(sfen).unwrap();
        for &(move_str, is_check) in move_candidates {
            let m = Move::new_from_usi_str(move_str, &pos);
            assert!(m.is_some());
            assert_eq!(pos.gives_check(m.unwrap()), is_check);
        }
    }
}

#[test]
fn test_position_do_move() {
    let sfen_and_moves_array = [
        ("4k4/9/9/9/9/9/9/9/4K4 b Bb 1", vec!["B*5g", "B*5c"]),
        (
            "lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1",
            vec![
                "7g7f", "3c3d", "2g2f", "5c5d", "5g5f", "2b8h+", "7i8h", "B*5g", "B*5c", "8b5b",
                "5c8f+", "5a6b", "3i4h", "5g2d+", "8h7g", "5d5e", "2f2e", "2d3e", "5f5e", "5b5e",
                "P*5g", "7a7b", "7g6f", "5e5a", "3g3f", "3e4d", "2e2d", "2c2d", "2h2d", "3a3b",
                "5i6h", "6b7a", "4g4f", "P*5f", "5g5f", "5a5f", "4i5h", "P*2c", "2d2g", "5f5h+",
                "6i5h", "G*8h", "8i7g", "8h9i", "7g6e", "L*5a", "P*5e", "5a5e", "5h4g", "P*5f",
                "P*5h", "9i9h", "2g2h", "4a5b", "R*3a",
            ],
        ),
    ];
    for (sfen, moves) in sfen_and_moves_array.iter() {
        let mut pos = Position::new_from_sfen(sfen).unwrap();
        for move_str in moves {
            let m = Move::new_from_usi_str(move_str, &pos);
            assert!(m.is_some());
            let m = m.unwrap();
            let gives_check = pos.gives_check(m);
            {
                // Checking Position::do_move and Position::undo_move work accurately.
                // (Position::do_move and Position::undo_move call is_ok().)
                pos.do_move(m, gives_check);
                pos.undo_move(m);
            }
            pos.do_move(m, gives_check);
            assert!(pos.is_repetition() == Repetition::Not);
        }
    }
}

#[test]
fn test_check_info_new() {
    // CheckInfo::check_squares in CheckInfo::new() depends on the following assumptions.
    assert_eq!(0, PieceType::OCCUPIED.0);
    assert_eq!(1, PieceType::PAWN.0);
    assert_eq!(2, PieceType::LANCE.0);
    assert_eq!(3, PieceType::KNIGHT.0);
    assert_eq!(4, PieceType::SILVER.0);
    assert_eq!(5, PieceType::BISHOP.0);
    assert_eq!(6, PieceType::ROOK.0);
    assert_eq!(7, PieceType::GOLD.0);
    assert_eq!(8, PieceType::KING.0);
    assert_eq!(9, PieceType::PRO_PAWN.0);
    assert_eq!(10, PieceType::PRO_LANCE.0);
    assert_eq!(11, PieceType::PRO_KNIGHT.0);
    assert_eq!(12, PieceType::PRO_SILVER.0);
    assert_eq!(13, PieceType::HORSE.0);
    assert_eq!(14, PieceType::DRAGON.0);
}

#[test]
fn test_check_info_do_move() {
    let sfen = "9/4R+P2k/9/9/9/9/9/8K/9 b - 1";
    let mut pos = Position::new_from_sfen(sfen).unwrap();
    let move_str = "4b4a";
    let m = Move::new_from_usi_str(move_str, &pos).unwrap();
    let gives_check = pos.gives_check(m);
    assert!(gives_check);
    pos.do_move(m, gives_check);
    assert!(pos.checkers().is_set(Square::SQ52));
}

#[test]
fn test_huffman_code() {
    let pos = Position::new_from_sfen(START_SFEN).unwrap();
    let hcp = HuffmanCodedPosition::from(&pos);
    match Position::new_from_huffman_coded_position(&hcp) {
        Ok(pos_from_hcp) => {
            let sfen = pos_from_hcp.to_sfen();
            assert_eq!(START_SFEN, &sfen);
        }
        Err(_err) => {
            assert!(false);
        }
    }
}

#[test]
fn test_is_entering_king_win() {
    const STACK_SIZE: usize = 128 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let pos = Position::new_from_sfen("1p7/KRRBBPPPP/NN7/9/9/9/9/9/8k b 2P 1").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("1p7/KRRBBPPPP/NN7/9/9/9/9/9/8k w 2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // opponent side is entring king position. but own side is not.
            let pos = Position::new_from_sfen("pp7/KRRBBPPPP/NN7/9/9/9/9/9/8k b 2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // in_check
            let pos = Position::new_from_sfen("1p7/1RRBBPPPP/NNN6/K8/9/9/9/9/8k b 2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // not entering king
            let pos = Position::new_from_sfen("1p7/KRRBBPPPP/N8/9/9/9/9/9/8k b 3P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 10 own pieces on the opponent field.
            let pos = Position::new_from_sfen("1p7/KRRBBPPPP/N8/N8/9/9/9/9/8k b 2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 28 point.
            let pos =
                Position::new_from_sfen("1pGGGGS2/KRRB1PPPP/N8/N8/9/9/9/9/8k b 2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 28 point.

            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbbpppp/1P7 w p 2").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbbpppp/1P7 b p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // opponent side is entring king position. but own side is not.
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbbpppp/PP7 w p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // in_check
            let pos = Position::new_from_sfen("K8/9/9/9/9/k8/nn7/1rrbbpppp/1P7 w p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // not entering king
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/n8/krrbbpppp/1P7 w 2p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 10 own pieces on the opponent field.
            let pos = Position::new_from_sfen("K8/9/9/9/9/n8/n8/krrbbpppp/1P7 w p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 27 point.
            let pos = Position::new_from_sfen("K8/9/9/9/9/n8/n8/krrb1pppp/1Pggggs2 w p 2").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 27 point.

            // check point of hand big pieces
            let pos = Position::new_from_sfen("1p7/KRRBPPPPP/NN7/9/9/9/9/9/8k b BP 1").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("1p7/KR+RB+PPPPP/NN7/9/9/9/9/9/8k b BP 1").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("1p7/KRRBPPPPP/NN7/9/9/9/9/9/8k w BP 1").unwrap();
            assert!(!pos.is_entering_king_win()); // opponent side is entring king position. but own side is not.
            let pos = Position::new_from_sfen("pp7/KRRBPPPPP/NN7/9/9/9/9/9/8k b BP 1").unwrap();
            assert!(!pos.is_entering_king_win()); // in_check
            let pos = Position::new_from_sfen("1p7/1RRBPPPPP/NNN6/K8/9/9/9/9/8k b BP 1").unwrap();
            assert!(!pos.is_entering_king_win()); // not entering king
            let pos = Position::new_from_sfen("1p7/KRRBPPPPP/N8/9/9/9/9/9/8k b B2P 1").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 10 own pieces on the opponent field.
            let pos =
                Position::new_from_sfen("1pGGGGS2/KR1BPPPPP/N8/N8/9/9/9/9/8k b BP 1").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 28 point.

            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbppppp/1P7 w b 2").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/kr+rb+ppppp/1P7 w b 2").unwrap();
            assert!(pos.is_entering_king_win());
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbppppp/1P7 b b 1").unwrap();
            assert!(!pos.is_entering_king_win()); // opponent side is entring king position. but own side is not.
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/nn7/krrbppppp/PP7 w b 2").unwrap();
            assert!(!pos.is_entering_king_win()); // in_check
            let pos = Position::new_from_sfen("K8/9/9/9/9/k8/nnn6/1rrbppppp/1P w b 2").unwrap();
            assert!(!pos.is_entering_king_win()); // not entering king
            let pos = Position::new_from_sfen("K8/9/9/9/9/9/n8/krrbppppp/1P7 w bp 2").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 10 own pieces on the opponent field.
            let pos = Position::new_from_sfen("K8/9/9/9/9/n8/n8/kr1bppppp/1Pggggs2 w b 2").unwrap();
            assert!(!pos.is_entering_king_win()); // less than 27 point.
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn test_pseudo_legal() {
    let sfen = "4k4/4l4/9/9/4K4/9/9/9/9 b - 1";
    let pos = Position::new_from_sfen(sfen).unwrap();
    assert!(!pos.pseudo_legal::<SearchingType>(Move::new_unpromote(
        Square::SQ55,
        Square::SQ56,
        Piece::B_KING
    )));
}

#[test]
fn test_is_repetition() {
    const STACK_SIZE: usize = 128 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let sfen = "8k/9/9/9/9/9/9/9/8K b R2P 1";
            let moves = [
                ("P*1b", Repetition::Not),
                ("1a2a", Repetition::Not),
                ("1b1a+", Repetition::Not),
                ("2a1a", Repetition::Inferior),
                ("P*1b", Repetition::Superior),
                ("1a2a", Repetition::Inferior),
                ("R*2b", Repetition::Not),
                ("2a3a", Repetition::Not),
                ("2b3b", Repetition::Not),
                ("3a2a", Repetition::Not),
                ("3b2b", Repetition::Win),
                ("2a3a", Repetition::Lose),
            ];
            let mut pos = Position::new_from_sfen(sfen).unwrap();
            for (m, r) in &moves {
                let m = Move::new_from_usi_str(m, &pos).unwrap();
                pos.do_move(m, pos.gives_check(m));
                assert_eq!(pos.is_repetition(), *r);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn test_mate_move_in_1ply() {
    const STACK_SIZE: usize = 128 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let sfen = "8k/9/8P/9/9/9/9/9/8K b G 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "G*1b");

            let sfen = "8k/9/9/9/9/9/9/9/8K b G 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7bk/9/8P/9/9/9/9/9/8K b G 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "6Rbk/9/8P/9/9/9/9/9/8K b G 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "G*1b");

            let sfen = "8k/9/8P/9/9/9/9/9/8K b L 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7nk/7n1/8P/9/9/9/9/9/8K b L 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "L*1b");

            let sfen = "7nk/7n1/8P/9/9/9/9/9/8K b RL 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "R*1b"); // Rook is checked before Lance.

            let sfen = "7k1/R8/9/9/9/9/9/9/8K b S 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7pk/7bp/9/9/9/9/9/9/8K b N 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "N*2c");

            let sfen = "7pk/7bs/9/9/9/9/9/8L/8K b N 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "N*2c");

            let sfen = "7pk/7bs/9/9/9/9/9/9/8K b N 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7pk/7nn/9/9/8N/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "1e2c");

            let sfen = "7pk/7nn/9/8s/8N/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7pk/7nn/9/8l/8N/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "8k/7nn/9/9/8N/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7nk/7pn/9/9/8N/9/9/9/B7K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "1e2c");

            let sfen = "8k/9/8P/8L/9/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "1c1b+");

            let sfen = "7k1/9/7P1/7L1/9/9/9/9/1K7 b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "2c2b+");

            let sfen = "7k1/8g/7P1/7L1/9/9/9/9/1K7 b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7k1/8b/7P1/7L1/9/9/9/9/1K7 b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7p1/7lk/7ll/8L/9/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "7p1/7lk/7ll/7BL/9/9/9/9/8K b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "1d1c");

            let sfen = "ln5nl/4g2G1/pr1p1skpp/2P2psR1/1SpPp3B/Pp4G1P/N3PbN2/2G6/L3K3L b Ps4p 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "2d2c+");

            let sfen =
                "+L7R/3pp4/1bSk5/+B2+n3n1/1K1L1s3/1PG6/2+ng1+n2P/2+p6/1+pL2+p1+p1 b R3P2g2sl7p 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "3+rn4/RP6+S/2p2+Pp1p/4P2k1/4K+P3/p1P+p2PP1/+p4+pB2/S+p1+p+n+b2P/4S3+p w 3G2N2Lgs2l 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "4l1l1p/G+P2+L4/2+P1+PpS+S+S/l3p1K2/1G+p3+P1k/+p+bp1+p2P+p/5g2+p/G2+BP2S1/1+p1+p1PN2 w RNr2n 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "2g1+Pp3/+P2+Pn1g2/1gkLK1n2/9/+P1+PS1P3/pl1P2+p2/1LP1N+s+pp1/PP2P2SL/N2S2PPg b 2R2B 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_none());

            let sfen = "5pp2/prp1l2+Pl/1SSpnk3/1K7/3+P+p2+P1/1P7/P1g+lpNR2/1p2PL2P/3+p1P1P1 b G2SN2b2gn 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let m = pos.mate_move_in_1ply();
            assert!(m.is_some());
            assert_eq!(&m.unwrap().to_usi_string(), "4g5e");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn test_effect_bb_of_checker_where_king_cannot_escape() {
    const STACK_SIZE: usize = 128 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let sfen = "4k4/4l4/9/9/4K4/9/9/9/9 b - 1";
            let pos = Position::new_from_sfen(sfen).unwrap();
            let bb = pos.effect_bb_of_checker_where_king_cannot_escape(
                Square::SQ52,
                pos.piece_on(Square::SQ52),
                &pos.occupied_bb(),
            );
            assert!(bb.is_set(Square::SQ56));
            assert!(bb.is_set(Square::SQ54));
        })
        .unwrap()
        .join()
        .unwrap();
}
