// Copyright (c) Facebook, Inc. and its affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the "hack" directory of this source tree.
//
// @generated SignedSource<<f0fba9df1394b12dae766886569aff2e>>
//
// To regenerate this file, run:
//   hphp/hack/src/oxidized_regen.sh

use arena_trait::TrivialDrop;
use eq_modulo_pos::EqModuloPos;
use no_pos_hash::NoPosHash;
use ocamlrep_derive::FromOcamlRepIn;
use ocamlrep_derive::ToOcamlRep;
use serde::Deserialize;
use serde::Serialize;

#[allow(unused_imports)]
use crate::*;

#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Eq,
    EqModuloPos,
    FromOcamlRepIn,
    Hash,
    NoPosHash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    ToOcamlRep
)]
#[repr(C, u8)]
pub enum Comment<'a> {
    #[serde(deserialize_with = "arena_deserializer::arena", borrow)]
    CmtLine(&'a str),
    #[serde(deserialize_with = "arena_deserializer::arena", borrow)]
    CmtBlock(&'a str),
}
impl<'a> TrivialDrop for Comment<'a> {}
arena_deserializer::impl_deserialize_in_arena!(Comment<'arena>);
