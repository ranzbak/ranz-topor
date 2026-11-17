#!/usr/bin/env bash
# refill-default fix round 2: targeted tests + python mutants (3 hzq units)
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
T="tests/plugin/test_refill_matrix.py tests/plugin/test_config.py tests/plugin/test_report.py tests/plugin/test_ui_wx.py tests/plugin/test_messages.py tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py tests/plugin/test_run_flow.py tests/test_docs_links.py"
hzq run --threads 4 --name rf2-tests -- bash -c "PYTHONPATH=plugin python -m pytest $T -q -p no:cacheprovider -rfEs > $R/pytest.txt 2>&1; echo rc=\$? >> $R/pytest.txt; PYTHONPATH=plugin python -m topor.docs_gen --check > $R/docs_gen.txt 2>&1; echo rc=\$? >> $R/docs_gen.txt" &
mu() { tag=$1; shift; hzq run --threads 4 --name rf2-mut$tag -- bash -c "T=/root/rf2-mut$tag; rm -rf \$T; mkdir -p \$T; tar --exclude=./target --exclude=./engine/target -cf - . | tar -xf - -C \$T; python tools/refill_r2_mut.py \$T $R/mut$tag $* > $R/mutants-$tag.txt 2>&1" & }
mu A M2_moot_by_setting_not_result M4_report_committed_wins N1_hint_always N2_hint_never
mu B N3_past_default_dropped N4_outcome_one_undo N5_dialog_one_undo N6_dialog_fallback_one_undo N7_fix_text_hint_in_catalogue
wait
echo "all done" > "$R/rc.txt"
