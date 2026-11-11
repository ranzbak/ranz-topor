"""Rule resolution (SWIG front end): the board's design settings and effective net classes are read with
pcbnew and handed to the pure resolver `toporlib.rules.resolve_rules_from_project` (the plugin feeds the
same function from IPC + the .kicad_pro). Board floors, netclasses, custom-rule (.kicad_dru) subset,
widths, JLCPCB profile."""
from __future__ import annotations

from harness.rip import NetWidths
from toporlib.dru import (  # noqa: F401  (re-export)
    _UNITS, DruRule, _match_term, _parse_sexp, _tokenize, _value_nm, match_condition, parse_dru,
)
from toporlib.rules import (  # noqa: F401  (re-export)
    JLCPCB_1OZ, NetClassVals, _POWER_RE, _REFUSE, _nz, resolve_rules_from_project,
)


def _swig_inputs(board) -> tuple[dict, dict, dict, dict]:
    """(pro, classes, netclass_of, net_codes) read from a pcbnew board. `pro` is synthesised from the
    design settings (mm); every effective (composite) class is registered under its own name with the
    values pcbnew reports, so the pure resolver needs no composition logic here."""
    ds = board.GetDesignSettings()
    ns = ds.m_NetSettings
    mm = lambda v: int(v) / 1e6  # noqa: E731
    pro = {"board": {"design_settings": {"rules": {
        "min_clearance": mm(ds.m_MinClearance),
        "min_track_width": mm(ds.m_TrackMinWidth),
        "min_via_diameter": mm(ds.m_ViasMinSize),
        "min_through_hole_diameter": mm(ds.m_MinThroughDrill),
        "min_via_annular_width": mm(ds.m_ViasMinAnnularWidth),
        "min_hole_clearance": mm(ds.m_HoleClearance),
        "min_hole_to_hole": mm(ds.m_HoleToHoleMin),
        "min_copper_edge_clearance": mm(ds.m_CopperEdgeClearance),
        "min_connection": mm(ds.m_MinConn),
        "max_error": mm(ds.m_MaxError),
    }, "via_dimensions": [{"diameter": mm(v.m_Diameter), "drill": mm(v.m_Drill)}
                          for v in ds.m_ViasDimensionsList]}}}
    # Violation severities live in the project file only (no stable SWIG accessor): read them from it.
    try:
        import json
        from pathlib import Path

        pro_p = Path(str(board.GetFileName())).with_suffix(".kicad_pro")
        if pro_p.is_file():
            sev = (json.loads(pro_p.read_text()).get("board", {}).get("design_settings", {}) or {}).get("rule_severities")
            if sev:
                pro["board"]["design_settings"]["rule_severities"] = sev
    except (OSError, ValueError):
        pass
    classes: dict[str, NetClassVals] = {}
    of: dict[str, str] = {}
    codes: dict[str, int] = {}
    names = [""]
    for code, n in sorted(board.GetNetsByNetcode().items()):
        if int(code) != 0:
            codes[str(n.GetNetname())] = int(code)
            names.append(str(n.GetNetname()))
    for name in names:
        nc = ns.GetEffectiveNetClass(name)
        nm = str(nc.GetName())
        of[name] = nm
        classes.setdefault(nm, NetClassVals(nm, int(nc.GetClearance()), int(nc.GetTrackWidth()),
                                            int(nc.GetViaDiameter()), int(nc.GetViaDrill())))
    return pro, classes, of, codes


def resolve_rules(
    board,
    *,
    dru_text: str | None,
    widths_before: dict[str, NetWidths],
    settings_table: dict[str, int] | None = None,
    profile: dict = JLCPCB_1OZ,
    apply_fab_floors: bool = True,
) -> dict:
    """Resolve design rules; manufacturer minimums (the profile) are FLOORS. See
    `toporlib.rules.resolve_rules_from_project` for the semantics (floors, `floors_applied`, warnings)."""
    pro, classes, of, codes = _swig_inputs(board)
    return resolve_rules_from_project(
        pro, dru_text, netclass_of=of, classes=classes, widths_before=widths_before,
        settings_table=settings_table, profile=profile, apply_fab_floors=apply_fab_floors,
        copper_layer_count=int(board.GetCopperLayerCount()), net_codes=codes,
        layer_names={int(l): str(board.GetLayerName(l)) for l in board.GetEnabledLayers().CuStack()})
