// Copyright (c) Facebook, Inc. and its affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the "hack" directory of this source tree.

use crate::{
    decl::{Tparam, Ty},
    reason::Reason,
};

use eq_modulo_pos::EqModuloPos;
use pos::TypeNameIndexMap;
use serde::{Deserialize, Serialize};

/// Maps type names to types with which to replace them.
#[derive(Debug, Clone, Eq, EqModuloPos, PartialEq, Serialize, Deserialize)]
#[serde(bound = "R: Reason")]
pub struct Subst<R: Reason>(pub TypeNameIndexMap<Ty<R>>);

impl<R: Reason> From<TypeNameIndexMap<Ty<R>>> for Subst<R> {
    fn from(map: TypeNameIndexMap<Ty<R>>) -> Self {
        Self(map)
    }
}

impl<R: Reason> From<Subst<R>> for TypeNameIndexMap<Ty<R>> {
    fn from(subst: Subst<R>) -> Self {
        subst.0
    }
}

impl<R: Reason> Subst<R> {
    pub fn new(tparams: &[Tparam<R, Ty<R>>], targs: &[Ty<R>]) -> Self {
        // If there are fewer type arguments than type parameters, we'll have
        // emitted an error elsewhere. We bind missing types to `Tany` (rather
        // than `Terr`) here to keep parity with the OCaml implementation, which
        // produces `Tany` because of a now-dead feature called "silent_mode".
        let targs = targs
            .iter()
            .cloned()
            .chain(std::iter::repeat(Ty::any(R::none())));
        Self(
            tparams
                .iter()
                .map(|tparam| tparam.name.id())
                .zip(targs)
                .collect(),
        )
    }
}
