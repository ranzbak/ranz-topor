set -u
W=$HZ_RUN_DIR/mutwork; mkdir -p $W; cp -r plugin tests docs pytest.ini $W/ 2>/dev/null; cd $W
run(){ PYTHONPATH=plugin python -m pytest tests/plugin/test_toml_defaults.py tests/plugin/test_config_docs.py tests/plugin/test_config.py -q -x 2>&1 | tail -1; }
echo "baseline: $(run)"
mut(){ name=$1; file=$2; from=$3; to=$4
  cp $file $HZ_RUN_DIR/orig.bak
  python - "$file" "$from" "$to" <<'PY'
import sys
f,a,b=sys.argv[1:]; t=open(f).read(); assert a in t, a; open(f,'w').write(t.replace(a,b,1))
PY
  echo "$name: $(run)"; cp $HZ_RUN_DIR/orig.bak $file; }
mut M1-all-real plugin/topor/config.py 'elif v == s.default:' 'elif False:'
mut M2-comment-user plugin/topor/config.py 'elif v == s.default:' 'elif True:'
mut M3-no-warning plugin/topor/config.py 'if pin:' 'if False:'
mut M4-warn-current plugin/topor/config.py 'if v == s.default:
        return None' 'if False:
        return None'
mut M5-drop-history-docs plugin/topor/docs_gen.py 'for (s, k), hist in PAST_DEFAULTS.items() for old, when in hist]' 'for (s, k), hist in {}.items() for old, when in hist]'
mut M6-change-value plugin/topor/config.py 'values[(sec, key)] = v
            if not err:' 'values[(sec, key)] = s.default if _pinned_old_default(s, v) else v
            if not err:'
mut M7-no-limitation plugin/topor/docs_gen.py 'Known limitation' 'Note'
mut M8-warn-twice plugin/topor/config.py 'findings.append(pin)' 'findings.extend([pin, pin])'
echo "mutants done"
