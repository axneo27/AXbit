use std::ops::{Add, Sub};

use nalgebra::SVector;


pub type Vec3d = cgmath::Vector3<f64>;

#[derive(Clone, Copy, Debug)]
pub struct StateVector {
    pub position: Vec3d,
    pub velocity: Vec3d,
}

impl StateVector {
    pub fn default() -> Self {
        StateVector {
            position: Vec3d::new(0.0, 0.0, 0.0),
            velocity: Vec3d::new(0.0, 0.0, 0.0),
        }
    }
}

impl From<[f64; 6]> for StateVector {
    fn from(arr: [f64; 6]) -> Self {
        StateVector {
            position: Vec3d::new(arr[0], arr[1], arr[2]),
            velocity: Vec3d::new(arr[3], arr[4], arr[5]),
        }
    }
}   

impl Into<[f64; 6]> for StateVector {
    fn into(self) -> [f64; 6] {
        [
            self.position.x,
            self.position.y,
            self.position.z,
            self.velocity.x,
            self.velocity.y,
            self.velocity.z,
        ]
    }
}

impl Add for StateVector {
    type Output = StateVector;

    fn add(self, other: StateVector) -> StateVector {
        StateVector {
            position: self.position + other.position,
            velocity: self.velocity + other.velocity,
        }
    }
}

impl Sub for StateVector {
    type Output = StateVector;

    fn sub(self, other: StateVector) -> StateVector {
        StateVector {
            position: self.position - other.position,
            velocity: self.velocity - other.velocity,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StateAtEpoch {
    pub state: StateVector,
    pub et: f64,
}

impl StateAtEpoch {
    pub fn default() -> Self {
        StateAtEpoch {
            state: StateVector::default(),
            et: 0.0,
        }
    }

    pub fn from_state_and_epoch(state: StateVector, et: f64) -> Self {
        StateAtEpoch { state, et }
    }

    pub fn from_svector_and_epoch(sv: SVector<f64, 6>, et: f64) -> Self {
        let state = StateVector::from([sv[0], sv[1], sv[2], sv[3], sv[4], sv[5]]);
        StateAtEpoch { state, et }
    }
}

impl From<[f64; 7]> for StateAtEpoch {
    fn from(arr: [f64; 7]) -> Self {
        StateAtEpoch {
            state: StateVector::from([arr[0], arr[1], arr[2], arr[3], arr[4], arr[5]]),
            et: arr[6],
        }
    }
}

impl Into<[f64; 7]> for StateAtEpoch {
    fn into(self) -> [f64; 7] {
        let mut arr: [f64; 7] = [0.0; 7];
        let state_arr: [f64; 6] = self.state.into();
        arr[..6].copy_from_slice(&state_arr);
        arr[6] = self.et;
        arr
    }
}