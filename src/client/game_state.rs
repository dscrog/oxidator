extern crate nalgebra as na;
use super::heightmap_editor;
use crate::frame::Frame;
use crate::mobile;
use crate::utils;
use na::{Matrix4, Point3, Vector2, Vector3};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use utils::*;

use crate::frame::Player;
use mobile::*;

pub struct State {
    pub position: Point3<f32>,
    pub dir: Vector3<f32>,

    pub position_smooth: Point3<f32>,
    pub dir_smooth: Vector3<f32>,

    pub mouse_world_pos: Option<Vector3<f32>>,
    pub screen_center_world_pos: Option<Vector3<f32>>,

    pub heightmap_editor: heightmap_editor::State,

    //Data to interpolate
    pub frame_minus_one: Frame,
    pub frame_zero: Frame,
    pub frame_zero_time_received: Instant,

    //Interpolated
    pub kbots: HashMap<Id<KBot>, KBot>,
    pub kinematic_projectiles: HashMap<Id<KinematicProjectile>, KinematicProjectile>,
    pub server_sec: f32,

    pub in_screen: Vec<(Id<KBot>, Vector2<f32>)>,
    pub selected: HashSet<IdValue>,

    pub start_time: Instant,
    pub last_frame: Instant,

    pub my_player_id: Option<Id<Player>>,

    pub players: HashMap<Id<Player>, Player>,

    pub fps: u64,
}

impl State {
    pub fn new() -> Self {
        State {
            position: Point3::new(1024.0, 100.0, 50.0),
            dir: Vector3::new(0.0, 0.3, -1.0),
            position_smooth: Point3::new(0.0, 0.0, 30000.0),
            dir_smooth: Vector3::new(0.0, 0.01, -1.0),

            mouse_world_pos: None,
            screen_center_world_pos: None,

            heightmap_editor: heightmap_editor::State::new(),

            frame_minus_one: Frame::new(),
            frame_zero: Frame::new(),
            frame_zero_time_received: Instant::now(),

            kbots: HashMap::new(),
            kinematic_projectiles: HashMap::new(),
            server_sec: 0.0,

            in_screen: Vec::new(),
            selected: HashSet::new(),

            players: HashMap::new(),
            my_player_id: None,

            start_time: Instant::now(),
            last_frame: Instant::now(),
            fps: 144,
        }
    }

    pub fn handle_new_frame(&mut self, frame: Frame) {
        let time_between = self.frame_zero_time_received.elapsed();
        log::trace!("receive: NewFrame after {:?}", time_between);
        self.frame_zero_time_received = Instant::now();
        self.frame_minus_one = std::mem::replace(&mut self.frame_zero, frame);

        self.selected = self
            .selected
            .difference(&self.frame_zero.kbots_dead.iter().map(|e| e.value).collect())
            .copied()
            .collect();
    }

    pub fn interpolate(&mut self, threadpool: &rayon::ThreadPool, view_proj: &Matrix4<f32>) {
        let elapsed = self.frame_zero_time_received.elapsed().as_secs_f64();
        //elapsed normalize between 0 and 1 if frame arrives every 100ms (0.1s)
        let lambda = (elapsed / 0.1) as f32;
        let i0 = lambda;
        let im = 1.0 - lambda;

        self.server_sec =
            (self.frame_zero.number as f32 * i0 + self.frame_minus_one.number as f32 * im) / 10.0;

        log::trace!("server_sec {}", self.server_sec);

        use rayon::prelude::*;

        self.in_screen.clear();

        fn test_screen(
            id: Id<KBot>,
            position: Point3<f32>,
            view_proj: &Matrix4<f32>,
        ) -> Option<(Id<KBot>, Vector2<f32>)> {
            let p = position.to_homogeneous();
            let r = view_proj * p;
            //Keeping those of the clipped space in screen (-1 1, -1 1 , 0 1)
            if r.z > 0.0 && r.x < r.w && r.x > -r.w && r.y < r.w && r.y > -r.w {
                Some((id, Vector2::new(r.x / r.w, r.y / r.w)))
            } else {
                None
            }
        }

        let mut kbots = self.frame_zero.kbots.clone();

        let mut in_screen: Vec<_> = Vec::new();
        threadpool.install(|| {
            in_screen = kbots
                .par_iter_mut()
                .map(|(_, kbot_0)| {
                    if let Some(kbot_m) = self.frame_minus_one.kbots.get(&kbot_0.id) {
                        let position = kbot_0.position * i0 + (im * kbot_m.position).coords;
                        let dir = kbot_0.dir * i0 + kbot_m.dir * im;
                        let mat = Matrix4::face_towards(
                            &position,
                            &(position + dir),
                            &Vector3::new(0.0, 0.0, 1.0),
                        );
                        let trans = Some(mat);
                        kbot_0.position = position;
                        kbot_0.dir = dir;
                        kbot_0.trans = trans;
                    } else {
                        let mat = Matrix4::face_towards(
                            &kbot_0.position,
                            &(kbot_0.position + kbot_0.dir),
                            &Vector3::new(0.0, 0.0, 1.0),
                        );

                        let trans = Some(mat);
                        kbot_0.trans = trans;
                    }

                    let screen = test_screen(kbot_0.id, kbot_0.position, view_proj);
                    if screen.is_some() {
                        kbot_0.is_in_screen = true;
                    }
                    screen
                })
                .collect();
        });

        self.kbots = kbots;

        self.in_screen = in_screen.iter().flatten().copied().collect();

        self.kinematic_projectiles.clear();
        for kine_0 in self.frame_zero.kinematic_projectiles.values() {
            let to_insert = {
                if let Some(kine_m) = self.frame_minus_one.kinematic_projectiles.get(&kine_0.id) {
                    let position = kine_0.position() * i0 + (im * kine_m.position()).coords;

                    let mut positions = vec![position];
                    positions.extend(&kine_0.positions.clone());
                    let kine = KinematicProjectile {
                        positions,
                        ..*kine_0
                    };

                    kine
                } else {
                    //No interpolation possible, taking last data point
                    kine_0.clone()
                }
            };

            self.kinematic_projectiles.insert(to_insert.id, to_insert);
        }

        // self.kbots = self.frame_zero.kbots.clone();
        // self.kinematic_projectiles = self.frame_zero.kinematic_projectiles.clone();
        self.players = self.frame_zero.players.clone();
    }

    pub fn my_player(&self) -> Option<&Player> {
        self.my_player_id
            .map(|id| self.players.get(&id))
            .unwrap_or(None)
    }
}