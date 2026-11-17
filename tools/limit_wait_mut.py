import sys
which=sys.argv[1]
p='engine/src/topo/core.rs'; s=open(p).read()
if which=='scope':
    s=s.replace("crate::control::in_loop_scope(|| optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock))","optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock)")
elif which.startswith('chk'):
    n=which[3]
    i=s.index(f'self.ctl.test_point("loop stage {n} check");')
    j=s.index("if stop() {",i)
    s=s[:j]+"if false && stop() {"+s[j+len("if stop() {"):]
elif which=='due':
    s=s.replace("reached && (wait_s > 0.0 || cycle_dropped)","reached && wait_s > 0.0")
elif which=='loopmsg':
    s=s.replace('let in_loop = step.contains("TopoR loop");','let in_loop = true;')
elif which=='latch':
    p='engine/src/topo/optimise.rs'; s=open(p).read(); s=s.replace("if hit || inner() {","if inner() {")
open(p,'w').write(s)
