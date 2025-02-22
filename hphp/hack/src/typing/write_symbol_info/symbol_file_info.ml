(*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the "hack" directory of this source tree.
 *
 *)

type t = {
  path: Relative_path.t;
  cst: Full_fidelity_positioned_syntax.t;
  tast: Tast.program;
  source_text: Full_fidelity_source_text.t;
}

let create ctx path =
  let (ctx, entry) = Provider_context.add_entry_if_missing ~ctx ~path in
  let source_text = Ast_provider.compute_source_text ~entry in
  let { Tast_provider.Compute_tast.tast; _ } =
    Tast_provider.compute_tast_unquarantined ~ctx ~entry
  in
  let cst =
    Provider_context.PositionedSyntaxTree.root
      (Ast_provider.compute_cst ~ctx ~entry)
  in
  { path; tast; source_text; cst }
