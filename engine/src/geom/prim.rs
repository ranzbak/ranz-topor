//! Geometric primitives: circle, segment, AABB, arc.
use super::vec2::{Orient, Vec2};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Circle {
    pub c: Vec2,
    pub r: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Seg {
    pub a: Vec2,
    pub b: Vec2,
}

impl Seg {
    pub fn len(&self) -> f64 {
        (self.b - self.a).len()
    }
    /// Unit direction `a → b`, `None` if degenerate.
    pub fn dir(&self) -> Option<Vec2> {
        (self.b - self.a).unit()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    pub fn grow(&self, d: f64) -> Aabb {
        Aabb {
            min: Vec2 {
                x: self.min.x - d,
                y: self.min.y - d,
            },
            max: Vec2 {
                x: self.max.x + d,
                y: self.max.y + d,
            },
        }
    }
    pub fn overlaps(&self, o: &Aabb) -> bool {
        self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb {
            min: Vec2 {
                x: self.min.x.min(o.min.x),
                y: self.min.y.min(o.min.y),
            },
            max: Vec2 {
                x: self.max.x.max(o.max.x),
                y: self.max.y.max(o.max.y),
            },
        }
    }
}

/// Circular arc; `sweep` in `[0, 2π)`, travelled in direction `sigma`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Arc {
    pub c: Vec2,
    pub r: f64,
    pub sigma: Orient,
    pub a_start: f64,
    pub sweep: f64,
}

impl Arc {
    pub fn point_at(&self, frac: f64) -> Vec2 {
        let a = self.a_start + self.sigma.sign() * self.sweep * frac;
        self.c
            + Vec2 {
                x: self.r * a.cos(),
                y: self.r * a.sin(),
            }
    }
    pub fn start(&self) -> Vec2 {
        self.point_at(0.0)
    }
    pub fn end(&self) -> Vec2 {
        self.point_at(1.0)
    }
    /// Angular midpoint (never the chord midpoint, 23 §4.5).
    pub fn mid(&self) -> Vec2 {
        self.point_at(0.5)
    }
    pub fn length(&self) -> f64 {
        self.r * self.sweep
    }
    pub fn sagitta(&self) -> f64 {
        self.r * (1.0 - (self.sweep * 0.5).cos())
    }
    pub fn chord(&self) -> f64 {
        2.0 * self.r * (self.sweep * 0.5).sin()
    }

    /// Whether the direction of `p` from the centre lies inside the angular span
    /// (cross-product rule of 23 §1, complement test for `sweep > π`).
    pub fn contains_dir(&self, p: Vec2) -> bool {
        let s = self.start() - self.c;
        let e = self.end() - self.c;
        let v = p - self.c;
        let sg = self.sigma.sign();
        if self.sweep <= PI {
            sg * s.cross(v) >= 0.0 && sg * v.cross(e) >= 0.0
        } else {
            // complement arc runs e → s with the same orientation
            !(sg * e.cross(v) > 0.0 && sg * v.cross(s) > 0.0)
        }
    }

    /// Arc on circle `(c, r, sigma)` from `t_in` to `t_out` (21 §3).
    pub fn from_tangent_points(c: Vec2, r: f64, sigma: Orient, t_in: Vec2, t_out: Vec2) -> Arc {
        let a_in = (t_in - c).angle();
        let a_out = (t_out - c).angle();
        let mut sweep = (sigma.sign() * (a_out - a_in)).rem_euclid(TAU);
        if sweep >= TAU {
            sweep = 0.0;
        }
        Arc {
            c,
            r,
            sigma,
            a_start: a_in,
            sweep,
        }
    }

    /// Circle through `s`, `m`, `e`; `None` if (nearly) collinear.
    pub fn from_three_points(s: Vec2, m: Vec2, e: Vec2) -> Option<Arc> {
        let u = m - s;
        let v = e - s;
        let cr = u.cross(v);
        if cr.abs() < 1e-6 * v.len2() {
            return None;
        }
        let u2 = u.len2();
        let v2 = v.len2();
        let off = Vec2 {
            x: v.y * u2 - u.y * v2,
            y: u.x * v2 - v.x * u2,
        } * (0.5 / cr);
        let c = s + off;
        // s→m→e turns left on the circle iff it runs counter-clockwise, for any sweep
        let sigma = if u.cross(e - m) > 0.0 {
            Orient::Ccw
        } else {
            Orient::Cw
        };
        let a_start = (s - c).angle();
        let mut sweep = (sigma.sign() * ((e - c).angle() - a_start)).rem_euclid(TAU);
        if sweep >= TAU {
            sweep = 0.0;
        }
        Some(Arc {
            c,
            r: off.len(),
            sigma,
            a_start,
            sweep,
        })
    }

    /// Endpoints plus axis extremes inside the span (23 §3).
    pub fn bbox(&self) -> Aabb {
        let s = self.start();
        let e = self.end();
        let mut bb = Aabb {
            min: Vec2 {
                x: s.x.min(e.x),
                y: s.y.min(e.y),
            },
            max: Vec2 {
                x: s.x.max(e.x),
                y: s.y.max(e.y),
            },
        };
        for k in 0..4 {
            let ang = k as f64 * FRAC_PI_2;
            let off = (self.sigma.sign() * (ang - self.a_start)).rem_euclid(TAU);
            if off <= self.sweep {
                let p = self.c
                    + Vec2 {
                        x: self.r * ang.cos(),
                        y: self.r * ang.sin(),
                    };
                bb.min = Vec2 {
                    x: bb.min.x.min(p.x),
                    y: bb.min.y.min(p.y),
                };
                bb.max = Vec2 {
                    x: bb.max.x.max(p.x),
                    y: bb.max.y.max(p.y),
                };
            }
        }
        bb
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Prim {
    Seg(Seg),
    Arc(Arc),
}

impl Prim {
    pub fn bbox(&self) -> Aabb {
        match self {
            Prim::Seg(s) => Aabb {
                min: Vec2 {
                    x: s.a.x.min(s.b.x),
                    y: s.a.y.min(s.b.y),
                },
                max: Vec2 {
                    x: s.a.x.max(s.b.x),
                    y: s.a.y.max(s.b.y),
                },
            },
            Prim::Arc(a) => a.bbox(),
        }
    }
    pub fn length(&self) -> f64 {
        match self {
            Prim::Seg(s) => s.len(),
            Prim::Arc(a) => a.length(),
        }
    }
}
