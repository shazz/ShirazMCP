//! Real format plugins (ADR-2): one file per format, registered by exactly
//! one line each in `registry::default_registry()`.

pub mod atomik;
pub mod atomik_bits;
pub mod atomik_decode;
pub mod boot_bpb;
pub mod boot_heur;
pub mod bootsector;
pub mod degas;
pub mod degas_rle;
pub mod ice_decode;
pub mod mpp;
pub mod mpp_decode;
pub mod neochrome;
pub mod pack_ice;
pub mod powerpacker;
pub mod pp_decode;
pub mod rnc;
pub mod rnc_m1;
pub mod rnc_m2;
pub mod sndh;
pub mod sndh_header;
pub mod ym_player;
pub mod ym_sig_db;
pub mod ym_signatures;
pub mod zx0;
pub mod zx0_decode;
