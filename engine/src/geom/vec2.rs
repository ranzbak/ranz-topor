//! 2-D vectors and orientation, `f64` nm, math axes (Y up).
use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2 {
            x: self.x + o.x,
            y: self.y + o.y,
        }
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2 {
            x: self.x - o.x,
            y: self.y - o.y,
        }
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2 {
            x: self.x * k,
            y: self.y * k,
        }
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2 {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl Vec2 {
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    /// `x·o.y − y·o.x`
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    /// 90° counter-clockwise: `(−y, x)`.
    pub fn perp(self) -> Vec2 {
        Vec2 {
            x: -self.y,
            y: self.x,
        }
    }
    pub fn rot(self, phi: f64) -> Vec2 {
        let (s, c) = phi.sin_cos();
        Vec2 {
            x: self.x * c - self.y * s,
            y: self.x * s + self.y * c,
        }
    }
    pub fn len2(self) -> f64 {
        self.dot(self)
    }
    pub fn len(self) -> f64 {
        self.len2().sqrt()
    }
    /// Unit vector, `None` if `len < 1e-9`.
    pub fn unit(self) -> Option<Vec2> {
        let l = self.len();
        if l < 1e-9 {
            None
        } else {
            Some(self * (1.0 / l))
        }
    }
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Orient {
    Ccw,
    Cw,
}

impl Orient {
    pub fn sign(self) -> f64 {
        match self {
            Orient::Ccw => 1.0,
            Orient::Cw => -1.0,
        }
    }
}
