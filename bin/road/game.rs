use std::collections::HashMap;
use std::f32::EPSILON;
use cgmath;
use cgmath::prelude::*;
use glutin::WindowEvent as Event;
use gfx;
use vangers::{config, level, model, render, space};


const MAX_TRACTION: config::common::Traction = 4.0;

#[derive(Debug)]
struct AccelerationVectors {
    f: cgmath::Vector3<f32>, // linear
    k: cgmath::Vector3<f32>, // angular
}

#[derive(Debug)]
struct CollisionPoint {
    pos: cgmath::Vector3<f32>,
    depth: f32,
}

#[derive(Debug)]
struct CollisionData {
    soft: Option<CollisionPoint>,
    hard: Option<CollisionPoint>,
}

struct Accumulator {
    pos: cgmath::Vector3<f32>,
    depth: f32,
    count: f32,
}

impl Accumulator {
    fn new() -> Accumulator {
        Accumulator {
            pos: cgmath::vec3(0.0, 0.0, 0.0),
            depth: 0.0,
            count: 0.0,
        }
    }
    fn add(&mut self, pos: cgmath::Vector3<f32>, depth: f32) {
        self.pos += pos;
        self.depth += depth;
        self.count += 1.0;
    }
    fn finish(&self, min: f32) -> Option<CollisionPoint> {
        if self.count > min {
            Some(CollisionPoint {
                pos: self.pos / self.count,
                depth: self.depth / self.count,
            })
        } else { None }
    }
}

#[derive(Eq, PartialEq)]
enum Spirit {
    Player,
    //Computer,
}

struct Dynamo {
    traction: config::common::Traction,
    _steer: cgmath::Rad<f32>,
    linear_velocity: cgmath::Vector3<f32>,
    angular_velocity: cgmath::Vector3<f32>,
}

impl Default for Dynamo {
    fn default() -> Dynamo {
        Dynamo {
            traction: 0.,
            _steer: cgmath::Rad(0.),
            linear_velocity: cgmath::Vector3::zero(),
            angular_velocity: cgmath::Vector3::zero(),
        }
    }
}

impl Dynamo {
    fn change_traction(&mut self, delta: config::common::Traction) {
        let old = self.traction;
        self.traction = (old + delta).min(MAX_TRACTION).max(-MAX_TRACTION);
        if old * self.traction < 0.0 {
            self.traction = 0.0; // full stop
        }
    }
}

struct Control {
    motor: f32,
    brake: bool,
    turbo: bool,
}

pub struct Agent<R: gfx::Resources> {
    spirit: Spirit,
    pub transform: space::Transform,
    pub car: config::car::CarInfo<R>,
    dynamo: Dynamo,
    control: Control,
}


fn get_height(altitude: u8) -> f32 {
    altitude as f32 * (level::HEIGHT_SCALE as f32) / 256.0
}

fn collide_low(poly: &model::Polygon, samples: &[[i8; 3]], scale: f32, transform: &space::Transform,
               level: &level::Level, terraconf: &config::common::Terrain) -> CollisionData
{
    let (mut soft, mut hard) = (Accumulator::new(), Accumulator::new());
    for &s in samples[poly.sample_range.0 as usize .. poly.sample_range.1 as usize].iter() {
        let sp = cgmath::Point3::from([s[0] as f32, s[1] as f32, s[2] as f32]);
        let pos = transform.transform_point(sp * scale).to_vec();
        let texel = level.get((pos.x as i32, pos.y as i32));
        let lo_alt = texel.low.0;
        let height = match texel.high {
            Some((delta, hi_alt, _)) => {
                let middle = get_height(lo_alt.saturating_add(delta));
                if pos.z > middle {
                    let high = get_height(hi_alt);
                    if pos.z - middle > high - pos.z {
                        high
                    } else {
                        continue
                    }
                } else {
                    get_height(lo_alt)
                }
            },
            None => get_height(lo_alt),
        };
        let dz = height - pos.z;
        //debug!("\t\t\tSample h={:?} at {:?}, dz={}", height, pos, dz);
        if dz > terraconf.min_wall_delta {
            //debug!("\t\t\tHard touch of {} at {:?}", dz, pos);
            hard.add(pos, dz);
        } else if dz > 0.0 {
            //debug!("\t\t\tSoft touch of {} at {:?}", dz, pos);
            soft.add(pos, dz);
        }
    }
    CollisionData {
        soft: (if soft.count > 0.0 { &soft } else { &hard }).finish(0.0),
        hard: hard.finish(4.0),
    }
}

fn _calc_collision_matrix_inv(r: cgmath::Vector3<f32>, ji: &cgmath::Matrix3<f32>) -> cgmath::Matrix3<f32> {
    let t3  = -r.z * ji[1][1] + r.y * ji[2][1];
    let t7  = -r.z * ji[1][2] + r.y * ji[2][2];
    let t12 = -r.z * ji[1][0] + r.y * ji[2][0];
    let t21 =  r.z * ji[0][1] - r.x * ji[2][1];
    let t25 =  r.z * ji[0][2] - r.x * ji[2][2];
    let t30 =  r.z * ji[0][0] - r.x * ji[2][0];
    let t39 = -r.y * ji[0][1] + r.x * ji[1][1];
    let t43 = -r.y * ji[0][2] + r.x * ji[1][2];
    let t48 = -r.y * ji[0][0] + r.x * ji[1][0];
    let cm = cgmath::Matrix3::new(
        1.0 - t3*r.z + t7*r.y, t12*r.z - t7*r.x, - t12*r.y + t3*r.x,
        - t21*r.z + t25*r.y, 1.0 + t30*r.z - t25*r.x, - t30*r.y + t21*r.x,
        - t39*r.z + t43*r.y, t48*r.z - t43*r.x, 1.0 - t48*r.y + t39*r.x
        );
    cm.invert().unwrap()
}

impl<R: gfx::Resources> Agent<R> {
    fn step(&mut self, dt: f32, level: &level::Level, common: &config::common::Common,
            mut line_buffer: Option<&mut render::LineBuffer>)
    {
        if self.control.motor != 0.0 {
            self.dynamo.change_traction(self.control.motor * dt * common.car.traction_incr);
        }
        if self.control.brake && self.dynamo.traction != 0.0 {
            self.dynamo.traction *= (config::common::ORIGINAL_FPS as f32 * -dt).exp2();
        }
        let acc_global = AccelerationVectors {
            f: cgmath::vec3(0.0, 0.0, -common.nature.gravity),
            k: cgmath::vec3(0.0, 0.0, 0.0),
        };
        let rot_inv = self.transform.rot.invert();
        debug!("dt {}, num {}", dt, common.nature.num_calls_analysis);
        let flood_level = level.flood_map[0] as f32;
        // Z axis in the local coordinate space
        let z_axis = rot_inv * cgmath::Vector3::unit_z();
        let mut v_vel = self.dynamo.linear_velocity;
        let mut w_vel = self.dynamo.angular_velocity;
        let j_inv = {
            let phys = &self.car.model.body.physics;
            (cgmath::Matrix3::from(phys.jacobi) *
                (self.transform.scale * self.transform.scale / phys.volume))
                .invert().unwrap()
        };

        let mut wheels_touch = 0u32;
        let mut spring_touch;

        /*for _ in 0 .. common.nature.num_calls_analysis*/ {
            let mut float_count = 0;
            let (mut terrain_immersion, mut water_immersion) = (0.0, 0.0);
            let stand_on_wheels = z_axis.z > 0.0 &&
                (self.transform.rot * cgmath::Vector3::unit_x()).z.abs() < 0.7;
            let modulation = 1.0;
            let mut acc_cur = AccelerationVectors {
                f: rot_inv * acc_global.f,
                k: rot_inv * acc_global.k,
            };

            // apply drag
            let mut v_drag = common.drag.free.v * common.drag.speed.v.powf(v_vel.magnitude());
            let mut w_drag = common.drag.free.w * common.drag.speed.w.powf(w_vel.magnitude2()); //why mag2?
            if wheels_touch > 0 { //TODO: why `ln()`?
                let speed = common.drag.wheel_speed.ln() * self.car.physics.mobility_factor *
                    common.global.speed_factor / self.car.physics.speed_factor;
                v_vel.y *= (1.0 + speed).powf(config::common::SPEED_CORRECTION_FACTOR);
            }
            wheels_touch = 0;
            spring_touch = 0;
            let mut down_minus_up = 0i32;
            let mut acc_springs = AccelerationVectors {
                f: cgmath::Vector3::zero(),
                k: cgmath::Vector3::zero(),
            };

            let mut sum_count = 0usize;
            let mut sum_rg0 = cgmath::Vector3::zero();
            let mut sum_df = 0.;

            for (bound_poly_id, poly) in self.car.model.shape.polygons.iter().enumerate() {
                let r = cgmath::Vector3::from(poly.middle) *
                    (self.transform.scale * self.car.physics.scale_bound);
                let rg0 = self.transform.rot * r;
                let rglob = rg0 + self.transform.disp;
                debug!("\t\tpoly[{}]: normal={:?} scale={} mid={:?} r={:?}", bound_poly_id,
                    poly.normal, self.transform.scale * self.car.physics.scale_bound, poly.middle, r);
                //let vr = v_vel + w_vel.cross(r);
                //let mostly_horisontal = vr.z*vr.z < vr.x*vr.x + vr.y*vr.y;
                let texel = level.get((rglob.x as i32, rglob.y as i32));
                if texel.low.1 == level::TerrainType::Water {
                    let dz = flood_level - rglob.z;
                    if dz > 0.0 {
                        float_count += 1;
                        water_immersion += dz;
                    }
                }
                let poly_norm = cgmath::Vector3::from(poly.normal).normalize();
                if z_axis.dot(poly_norm) < 0.0 {
                    let cdata = collide_low(poly, &self.car.model.shape.samples,
                        self.car.physics.scale_bound, &self.transform, level, &common.terrain);
                    debug!("\t\tcollide_low = {:?}", cdata);
                    terrain_immersion += match cdata.soft {
                        Some(ref cp) => cp.depth.abs(),
                        None => 0.0,
                    };
                    terrain_immersion += match cdata.hard {
                        Some(ref cp) => cp.depth.abs(),
                        None => 0.0,
                    };
                    /*let origin = self.transform.disp;
                    match cdata {
                        CollisionData{ hard: Some(ref cp), ..} if mostly_horisontal => {
                            let r1 = rot_inv * cgmath::vec3(
                                cp.pos.x - origin.x, cp.pos.y - origin.y, 0.0); // ignore vertical
                            let normal = {
                                let bm = self.car.model.body.bbox.1;
                                let n = cgmath::vec3(r1.x / bm[0], r1.y / bm[1], r1.z / bm[2]);
                                n.normalize()
                            };
                            let u0 = v_vel + w_vel.cross(r1);
                            let dot = u0.dot(normal);
                            if dot > 0.0 {
                                let pulse = (calc_collision_matrix_inv(r1, &j_inv) * normal) *
                                    (-common.impulse.factors[0] * modulation * dot);
                                debug!("\t\tCollision speed {:?} pulse {:?}", v_vel, pulse);
                                v_vel += pulse;
                                w_vel += j_inv * r1.cross(pulse);
                            }
                        },
                        CollisionData{ soft: Some(ref cp), ..} => {
                            let r1 = rot_inv * cgmath::vec3(cp.pos.x - origin.x, cp.pos.y - origin.y, rg0.z);
                            //TODO: let r1 = rot_inv * (cp.pos - origin);
                            let mut u0 = v_vel + w_vel.cross(r1);
                            debug!("\t\tContact {:?}\n\t\t\torigin={:?}\n\t\t\tu0 = {:?}", cp, origin, u0);
                            if u0.dot(z_axis) < 0.0 {
                                if stand_on_wheels { // ignore XY
                                    u0.x = 0.0;
                                    u0.y = 0.0;
                                } else {
                                    let kn = u0.dot(poly_norm) * (1.0 - common.impulse.k_friction);
                                    u0 = u0 * common.impulse.k_friction + poly_norm * kn;
                                }
                                let cmi = calc_collision_matrix_inv(r, &j_inv);
                                let pulse = (cmi * u0) * (-common.impulse.factors[1] * modulation);
                                debug!("\t\tCollision momentum {:?}\n\t\t\tmatrix {:?}\n\t\t\tsample {:?}\n\t\t\tspeed {:?}\n\t\t\tpulse {:?}",
                                    u0, cmi, r, v_vel, pulse);
                                v_vel += pulse;
                                w_vel += j_inv * r.cross(pulse);
                            }
                        }
                        _ => (),
                    }*/
                    if let Some(ref cp) = cdata.soft {
                        let df0 = common.contact.k_elastic_spring * cp.depth * modulation;
                        let df = df0.min(common.impulse.elastic_restriction);
                        debug!("\t\tbound[{}] dF.z = {}, rg0={:?}", bound_poly_id, df, rg0);
                        acc_springs.f.z += df;
					    acc_springs.k.x += rg0.y * df;
					    acc_springs.k.y -= rg0.x * df;
                        //let impulse = cgmath::vec3(0., 0., df);
                        //acc_springs.f += impulse;
                        //acc_springs.k += rg0.cross(impulse);
                        if stand_on_wheels {
                            wheels_touch += 1;
                        } else {
                            spring_touch += 1;
                        }
                        down_minus_up += 1;

                        sum_count += 1;
                        sum_rg0 += rg0;
                        sum_df += df;

                        if let Some(ref mut lbuf) = line_buffer {
                            lbuf.add(self.transform.disp.into(), rglob.into(), 0xFF000000);
                            let up = rglob + cgmath::vec3(0.0, 0.0, df0);
                            lbuf.add(rglob.into(), up.into(), 0xFFFF0000);
                        }
                    }
                } else {
                    //TODO: upper average
                    // down_minus_up -= 1;
                }
            }

            if sum_count != 0 {
                let kf = 1.0 / sum_count as f32;
                debug!("Avg df {} rg0 {:?}", sum_df * kf, sum_rg0 * kf);
            }

            if wheels_touch + spring_touch != 0 {
                debug!("\tsprings total {:?}", acc_springs);
                acc_cur.f += rot_inv * acc_springs.f;
                acc_cur.k += rot_inv * acc_springs.k;
            }

            let _ = (float_count, water_immersion, terrain_immersion); //TODO
            if wheels_touch != 0 && stand_on_wheels {
                let f_traction_per_wheel =
                    self.car.physics.mobility_factor * common.global.mobility_factor *
                    if self.control.turbo { common.global.k_traction_turbo } else { 1.0 } *
                    self.dynamo.traction / (self.car.model.wheels.len() as f32);
                for wheel in self.car.model.wheels.iter() {
                    let mut pos = cgmath::Vector3::from(wheel.pos) * self.transform.scale;
                    pos.x = pos.x.signum() * self.car.model.body.bbox.1[0]; // why?
                    acc_cur.f.y += f_traction_per_wheel;
                    if self.control.brake {
                        let vw = v_vel + w_vel.cross(pos);
                        acc_cur.f -= vw * common.global.f_brake_max;
                    }
                }
            }
            if spring_touch + wheels_touch != 0 {
                let tmp = cgmath::Vector3::new(0.0, 0.0,
                    self.car.physics.z_offset_of_mass_center * self.transform.scale);
                acc_cur.k -= common.nature.gravity * z_axis.cross(tmp);
                let vz = z_axis.dot(v_vel);
                if vz < -10.0 {
                    v_drag *= common.drag.z.powf(-vz);
                }
            }
            debug!("\tcur acc {:?}", acc_cur);
            v_vel += acc_cur.f * dt;
            w_vel += (j_inv * acc_cur.k) * dt;
            //debug!("J_inv {:?}, handedness {}", j_inv.transpose(), j_inv.x.cross(j_inv.y).dot(j_inv.z));
            debug!("\tresulting v={:?} w={:?}", v_vel, w_vel);
            if spring_touch != 0 {
                v_drag *= common.drag.spring.v;
                w_drag *= common.drag.spring.w;
            }
            let (v_mag, w_mag) = (v_vel.magnitude(), w_vel.magnitude());
            if stand_on_wheels && v_mag < common.drag.abs_min.v && w_mag < common.drag.abs_min.w {
                v_drag *= common.drag.coll.v.powf(common.drag.abs_min.v / (v_mag + EPSILON));
                w_drag *= common.drag.coll.w.powf(common.drag.abs_min.w / (w_mag + EPSILON));
            }
            if v_mag * v_drag > common.drag.abs_stop.v || w_mag * w_drag > common.drag.abs_stop.w {
                let vs = v_vel - (down_minus_up.signum() as f32) *
                    (z_axis * (self.car.model.body.bbox.2 * common.impulse.rolling_scale))
                    .cross(w_vel);
                let angle = cgmath::Rad(-dt * w_mag);
                let vel_rot_inv = cgmath::Quaternion::from_axis_angle(w_vel / (w_mag + EPSILON), angle);
                self.transform.disp += (self.transform.rot * vs) * dt;
                self.transform.rot = self.transform.rot * vel_rot_inv.invert();
                v_vel = vel_rot_inv * v_vel;
                w_vel = vel_rot_inv * w_vel;
                debug!("\tvs={:?} {:?}\n\t\tdisp {:?} scale {}", vs,
                    self.transform.rot, self.transform.disp, self.transform.scale);
            }
            //debug!("\tdrag v={} w={}", v_drag, w_drag);
            v_vel *= v_drag.powf(config::common::SPEED_CORRECTION_FACTOR);
            w_vel *= w_drag.powf(config::common::SPEED_CORRECTION_FACTOR);

            if let Some(ref mut lbuf) = line_buffer {
                let p0 = self.transform.disp + cgmath::vec3(0.0, 0.0, 10.0);
                let xf = p0 + acc_cur.f;
                let xk = p0 + acc_cur.k;
                lbuf.add(p0.into(), xf.into(), 0x0000FF00);
                lbuf.add(p0.into(), xk.into(), 0xFF00FF00);
                let xv = p0 + v_vel;
                let xw = p0 + w_vel * 10.0; //TEMP
                lbuf.add(p0.into(), xv.into(), 0x00FF0000);
                lbuf.add(p0.into(), xw.into(), 0x00FFFF00);
            }
        }

        /* dt 0.095507, num 5
        resulting v=(0.212911,8.957590,9.813052) w=(-0.865242,0.182465,0.072018)
        A_rot_inv=(0.999825, 0.006150, -0.017690, -0.007590, 0.996564, -0.082478, 0.017122, 0.082598, 0.996436)
        A_l2g=(0.714473, -0.661982, -0.226515, -0.658047, -0.745775, 0.103892, 0.237704, -0.074829, 0.968451)
        A_l2g_new=(0.714283, -0.646447, -0.268153, -0.664356, -0.746787, 0.030655, 0.220070, -0.156253, 0.962889)
        W new = (-0.865242, 0.182465, 0.072018)
        */
        if false {
            let w = cgmath::vec3(-0.865242,0.182465,0.072018);
            let wmag = w.magnitude();
            let a_l2g = cgmath::Matrix3::new(0.714473, -0.661982, -0.226515,
                0.237704, -0.074829, 0.968451,
                -0.658047, -0.745775, 0.103892)
                .transpose();
            let q_l2g = cgmath::Quaternion::from(a_l2g);
            let angle = cgmath::Rad(-0.095507 * wmag);
            let q_rot_inv = cgmath::Quaternion::from_axis_angle(w / (wmag + EPSILON), angle);
            let wnew = q_rot_inv * w;
            let q_l2g_new = q_l2g * q_rot_inv.invert();
            println!("\twnew: {:?}\n\ta_rot_inv: {:?}\n\tq_l2g: {:?}\n\tq_l2g_new: {:?}", wnew,
                cgmath::Matrix3::from(q_rot_inv).transpose(),
                cgmath::Matrix3::from(q_l2g).transpose(),
                cgmath::Matrix3::from(q_l2g_new).transpose());
        }
        /*
        wnew: Vector3 [-0.865242, 0.182465, 0.072018]
        a_rot_inv: Matrix3 [[0.9998246, 0.006150384, -0.017689865], [-0.007589605, 0.996564, -0.08247791], [0.01712181, 0.0825977, 0.9964359]]
        q_l2g: Matrix3 [[0.7144726, -0.66198176, -0.22651473], [0.23770414, -0.07482864, 0.96845084], [-0.6580467, -0.74577504, 0.10389289]]
        q_l2g_new: Matrix3 [[0.7142829, -0.6464473, -0.26815253], [0.22007042, -0.15625153, 0.9628885], [-0.664356, -0.7467872, 0.030656204]]
        */

        self.dynamo.linear_velocity  = v_vel;
        self.dynamo.angular_velocity = w_vel;
        // slow down
        let traction_step = -self.dynamo.traction.signum() * dt;
        self.dynamo.change_traction(traction_step * common.car.traction_decr);
    }
}

struct DataBase<R: gfx::Resources> {
    cars: HashMap<String, config::car::CarInfo<R>>,
    common: config::common::Common,
    _game: config::game::Registry,
}

pub struct Game<R: gfx::Resources> {
    db: DataBase<R>,
    render: render::Render<R>,
    line_buffer: render::LineBuffer,
    level: level::Level,
    agents: Vec<Agent<R>>,
    cam: space::Camera,
    spin_hor: f32,
    spin_ver: f32,
    is_paused: bool,
    tick: Option<f32>,
}

impl<R: gfx::Resources> Game<R> {
    pub fn new<F: gfx::Factory<R>>(settings: &config::Settings,
           out_color: gfx::handle::RenderTargetView<R, render::ColorFormat>,
           out_depth: gfx::handle::DepthStencilView<R, render::DepthFormat>,
           factory: &mut F) -> Game<R>
    {
        info!("Loading world parameters");
        let db = {
            let game = config::game::Registry::load(settings);
            DataBase {
                cars: config::car::load_registry(settings, &game, factory),
                common: config::common::load(settings.open("common.prm")),
                _game: game,
            }
        };
        let level = match settings.get_level() {
            Some(lev_config) => level::load(&lev_config),
            None => level::Level::new_test(),
        };
        let pal_data = level::load_palette(&settings.get_object_palette_path());
        let car = db.cars[&settings.car.id].clone();
        let player_height = get_height(level.get((0, 0)).get_top()) + 5.; //center offset

        let agent = Agent {
            spirit: Spirit::Player,
            transform: cgmath::Decomposed {
                scale: car.scale,
                disp: cgmath::vec3(0.0, 0.0, player_height),
                rot: cgmath::One::one(),
            },
            car: car,
            dynamo: Dynamo::default(),
            control: Control {
                motor: 0.0,
                brake: false,
                turbo: false,
            },
        };

        Game {
            db: db,
            render: render::init(factory, out_color, out_depth, &level, &pal_data),
            line_buffer: render::LineBuffer::new(),
            level: level,
            agents: vec![agent],
            cam: space::Camera {
                loc: cgmath::vec3(0.0, 0.0, 200.0),
                rot: cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0),
                proj: cgmath::PerspectiveFov {
                    fovy: cgmath::Deg(45.0).into(),
                    aspect: settings.get_screen_aspect(),
                    near: 10.0,
                    far: 10000.0,
                },
            },
            spin_hor: 0.0,
            spin_ver: 0.0,
            is_paused: false,
            tick: None,
        }
    }

    fn _move_cam(&mut self, step: f32) {
        let mut back = self.cam.rot * cgmath::Vector3::unit_z();
        back.z = 0.0;
        self.cam.loc -= back.normalize() * step;
    }
}

impl<R: gfx::Resources> Game<R> {
    pub fn react<F>(&mut self, event: Event, factory: &mut F)
                 -> bool where F: gfx::Factory<R>
    {
        use glutin::VirtualKeyCode as Key;
        use glutin::ElementState::*;

        let player = match self.agents.iter_mut().find(|a| a.spirit == Spirit::Player) {
            Some(agent) => agent,
            None => return false,
        };
        match event {
            Event::KeyboardInput(Pressed, _, Some(Key::Escape), _) |
            Event::Closed => return false,
            //Event::Resized(width, height) => self.render.resize(width, height),
            Event::KeyboardInput(Pressed, _, Some(Key::L), _) => self.render.reload(factory),
            Event::KeyboardInput(Pressed, _, Some(Key::P), _) => {
                let center = &player.transform;
                self.tick = None;
                if self.is_paused {
                    self.is_paused = false;
                    self.cam.loc = center.disp + cgmath::vec3(0.0, 0.0, 200.0);
                    self.cam.rot = cgmath::Quaternion::new(1.0, 0.0, 0.0, 0.0);
                } else {
                    self.is_paused = true;
                    self.cam.focus_on(center);
                }
            },
            Event::KeyboardInput(Pressed, _, Some(Key::Comma), _) => self.tick = Some(-1.0),
            Event::KeyboardInput(Pressed, _, Some(Key::Period), _) => self.tick = Some(1.0),
            /*
            Event::KeyboardInput(_, _, Some(Key::R)) =>
                self.cam.rot = self.cam.rot * cgmath::Quaternion::from_axis_angle(cgmath::Vector3::unit_x(), angle),
            Event::KeyboardInput(_, _, Some(Key::F)) =>
                self.cam.rot = self.cam.rot * cgmath::Quaternion::from_axis_angle(cgmath::Vector3::unit_x(), -angle),
            */
            Event::KeyboardInput(Pressed, _, Some(Key::W), _) => self.spin_ver = 1.0,
            Event::KeyboardInput(Pressed, _, Some(Key::S), _) => self.spin_ver = -1.0,
            Event::KeyboardInput(Released, _, Some(Key::W), _) |
            Event::KeyboardInput(Released, _, Some(Key::S), _) =>
                self.spin_ver = 0.0,
            Event::KeyboardInput(Pressed, _, Some(Key::R), _) => {
                player.transform.rot = cgmath::One::one();
                player.dynamo.linear_velocity = cgmath::Vector3::zero();
                player.dynamo.angular_velocity = cgmath::Vector3::zero();
            },
            Event::KeyboardInput(Pressed, _, Some(Key::A), _) => self.spin_hor = -1.0,
            Event::KeyboardInput(Pressed, _, Some(Key::D), _) => self.spin_hor = 1.0,
            Event::KeyboardInput(Released, _, Some(Key::A), _) |
            Event::KeyboardInput(Released, _, Some(Key::D), _) =>
                self.spin_hor = 0.0,
            /*
            Event::KeyboardInput(_, _, Some(Key::W)) => self.move_cam(step),
            Event::KeyboardInput(_, _, Some(Key::S)) => self.move_cam(-step),
            Event::KeyboardInput(_, _, Some(Key::A)) =>
                self.cam.rot = cgmath::Quaternion::from_axis_angle(cgmath::Vector3::unit_z(), angle) * self.cam.rot,
            Event::KeyboardInput(_, _, Some(Key::D)) =>
                self.cam.rot = cgmath::Quaternion::from_axis_angle(cgmath::Vector3::unit_z(), -angle) * self.cam.rot,
            */
            _ => {},
        }

        true
    }

    pub fn update(&mut self, delta: f32) {
        //let dt = delta * config::common::SPEED_CORRECTION_FACTOR;
        //let dt = delta * 6.0;//TODO
        let dt = 0.093912; //TODO
        let pid = self.agents.iter().position(|a| a.spirit == Spirit::Player).unwrap();

        if self.is_paused {
            let player = &mut self.agents[pid];
            if let Some(tick) = self.tick.take() {
                self.line_buffer.clear();
                player.step(tick * dt, &self.level, &self.db.common, Some(&mut self.line_buffer));
            }
            self.cam.rotate_focus(&player.transform,
                cgmath::Rad(2.0 * delta * self.spin_hor),
                cgmath::Rad(delta * self.spin_ver));
        } else {
            //self.dyn_target.steer += cgmath::Rad(0.2 * delta * self.spin_hor);
            self.agents[pid].control.motor = 1.0 * self.spin_ver;

            self.cam.look_by(&self.agents[pid].transform, &space::Direction {
                view: cgmath::vec3(0.0, 1.0, -3.0),
                height: 200.0,
            });
            self.line_buffer.clear();

            for a in self.agents.iter_mut() {
                a.step(dt, &self.level, &self.db.common, Some(&mut self.line_buffer));
            }
        }
    }

    pub fn draw<C: gfx::CommandBuffer<R>>(&mut self, enc: &mut gfx::Encoder<R, C>) {
        let items = self.agents.iter().map(|a|
            (&a.car.model, &a.transform, a.car.physics.scale_bound)
        );
        self.render.draw_world(enc, items, &self.cam, false);
        self.render.debug.draw_lines(&self.line_buffer, self.cam.get_view_proj().into(), enc);
    }
}