use material::Material;
use math::align_to;
use vmath::{dot, vec3, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

#[inline]
pub fn ray(origin: Vec3, direction: Vec3) -> Ray {
    Ray { origin, direction }
}

impl Ray {
    #[inline]
    #[allow(dead_code)]
    pub fn new(origin: Vec3, direction: Vec3) -> Ray {
        Ray { origin, direction }
    }
    #[inline]
    pub fn point_at_parameter(&self, t: f32) -> Vec3 {
        self.origin + (t * self.direction)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub point: Vec3,
    pub normal: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct Sphere {
    pub centre: Vec3,
    pub radius: f32,
}

#[inline]
pub fn sphere(centre: Vec3, radius: f32, material: Material) -> (Sphere, Material) {
    (Sphere { centre, radius }, material)
}

#[derive(Debug)]
pub struct SpheresSoA {
    len: usize,
    centre_x: Vec<f32>,
    centre_y: Vec<f32>,
    centre_z: Vec<f32>,
    radius_sq: Vec<f32>,
    radius_inv: Vec<f32>,
    material: Vec<Material>,
}

impl SpheresSoA {
    pub fn new(sphere_materials: &[(Sphere, Material)]) -> SpheresSoA {
        // HACK: make sure there's enough entries for SIMD
        // TODO: conditionally compile this
        let unaligned_len = sphere_materials.len();
        let len = align_to(unaligned_len, 4);
        let mut centre_x = Vec::with_capacity(len);
        let mut centre_y = Vec::with_capacity(len);
        let mut centre_z = Vec::with_capacity(len);
        let mut radius_inv = Vec::with_capacity(len);
        let mut radius_sq = Vec::with_capacity(len);
        let mut material = Vec::with_capacity(len);
        for (sphere, mat) in sphere_materials {
            centre_x.push(sphere.centre.x);
            centre_y.push(sphere.centre.y);
            centre_z.push(sphere.centre.z);
            radius_sq.push(sphere.radius * sphere.radius);
            radius_inv.push(1.0 / sphere.radius);
            material.push(*mat);
        }
        let padding = len - unaligned_len;
        for _ in 0..padding {
            centre_x.push(0.0);
            centre_y.push(0.0);
            centre_z.push(0.0);
            radius_sq.push(0.0);
            radius_inv.push(1.0);
            material.push(Material::Lambertian {
                albedo: vec3(0.0, 0.0, 0.0),
            });
        }
        SpheresSoA {
            len,
            centre_x,
            centre_y,
            centre_z,
            radius_sq,
            radius_inv,
            material,
        }
    }

    pub fn hit(&self, ray: &Ray, t_min: f32, t_max: f32) -> Option<(RayHit, &Material)> {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if is_x86_feature_detected!("sse4.1") {
                return unsafe { self.hit_sse4_1(ray, t_min, t_max) };
            }
        }

        self.hit_scalar(ray, t_min, t_max)
    }

    fn hit_scalar(&self, ray: &Ray, t_min: f32, t_max: f32) -> Option<(RayHit, &Material)> {
        let mut hit_t = t_max;
        let mut hit_index = self.len;
        for ((((index, centre_x), centre_y), centre_z), radius_sq) in self
            .centre_x
            .iter()
            .enumerate()
            .zip(self.centre_y.iter())
            .zip(self.centre_z.iter())
            .zip(self.radius_sq.iter())
        {
            let co = vec3(
                centre_x - ray.origin.x,
                centre_y - ray.origin.y,
                centre_z - ray.origin.z,
            );
            let nb = dot(co, ray.direction);
            let c = dot(co, co) - radius_sq;
            let discriminant = nb * nb - c;
            if discriminant > 0.0 {
                let discriminant_sqrt = discriminant.sqrt();
                let mut t = nb - discriminant_sqrt;
                if t < t_min {
                    t = nb + discriminant_sqrt;
                }
                if t > t_min && t < hit_t {
                    hit_t = t;
                    hit_index = index;
                }
            }
        }
        if hit_index < self.len {
            let point = ray.point_at_parameter(hit_t);
            let normal = vec3(
                point.x - self.centre_x[hit_index],
                point.y - self.centre_y[hit_index],
                point.z - self.centre_z[hit_index],
            ) * self.radius_inv[hit_index];
            let material = &self.material[hit_index];
            Some((RayHit { point, normal }, material))
        } else {
            None
        }
    }

    #[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), target_feature(enable = "sse4.1"))]
    unsafe fn hit_sse4_1(&self, ray: &Ray, t_min: f32, t_max: f32) -> Option<(RayHit, &Material)> {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::*;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::*;
        let t_min = _mm_set_ps1(t_min);
        let mut hit_t = _mm_set_ps1(t_max);
        let mut hit_index = _mm_set_epi32(-1, -1, -1, -1);
        // load ray origin
        let ro_x = _mm_set_ps1(ray.origin.x);
        let ro_y = _mm_set_ps1(ray.origin.y);
        let ro_z = _mm_set_ps1(ray.origin.z);
        // load ray direction
        let rd_x = _mm_set_ps1(ray.direction.x);
        let rd_y = _mm_set_ps1(ray.direction.y);
        let rd_z = _mm_set_ps1(ray.direction.z);
        // current indices being processed (little endian ordering)
        let mut index = _mm_set_epi32(3, 2, 1, 0);
        // loop over 4 spheres at a time
        for (((centre_x, centre_y), centre_z), radius_sq) in self
            .centre_x
            .chunks(4)
            .zip(self.centre_y.chunks(4))
            .zip(self.centre_z.chunks(4))
            .zip(self.radius_sq.chunks(4))
        {
            // load sphere centres
            // TODO: align memory
            let c_x = _mm_loadu_ps(centre_x.as_ptr());
            let c_y = _mm_loadu_ps(centre_y.as_ptr());
            let c_z = _mm_loadu_ps(centre_z.as_ptr());
            // load radius_sq
            let r_sq = _mm_loadu_ps(radius_sq.as_ptr());
            // let co = centre - ray.origin
            let co_x = _mm_sub_ps(c_x, ro_x);
            let co_y = _mm_sub_ps(c_y, ro_y);
            let co_z = _mm_sub_ps(c_z, ro_z);
            // TODO: write a dot product helper
            // let nb = dot(co, ray.direction);
            let nb = _mm_mul_ps(co_x, rd_x);
            let nb = _mm_add_ps(nb, _mm_mul_ps(co_y, rd_y));
            let nb = _mm_add_ps(nb, _mm_mul_ps(co_z, rd_z));
            // let c = dot(co, co) - radius_sq;
            let c = _mm_mul_ps(co_x, co_x);
            let c = _mm_add_ps(c, _mm_mul_ps(co_y, co_y));
            let c = _mm_add_ps(c, _mm_mul_ps(co_z, co_z));
            let c = _mm_sub_ps(c, r_sq);
            // let discriminant = nb * nb - c;
            let discr = _mm_sub_ps(_mm_mul_ps(nb, nb), c);
            // if discr > 0.0
            let pos_discr = _mm_cmpgt_ps(discr, _mm_set_ps1(0.0));
            if _mm_movemask_ps(pos_discr) != 0 {
                // let discr_sqrt = discr.sqrt();
                let discr_sqrt = _mm_sqrt_ps(discr);
                // let t0 = nb - discr_sqrt;
                // let t1 = nb + discr_sqrt;
                let t0 = _mm_sub_ps(nb, discr_sqrt);
                let t1 = _mm_add_ps(nb, discr_sqrt);
                // let t = if t0 > t_min { t0 } else { t1 };
                let t = _mm_blendv_ps(t1, t0, _mm_cmpgt_ps(t0, t_min));
                // from rygs opts
                // bool4 msk = discrPos & (t > tMin4) & (t < hitT);
                let mask = _mm_and_ps(
                    pos_discr,
                    _mm_and_ps(_mm_cmpgt_ps(t, t_min), _mm_cmplt_ps(t, hit_t)),
                );
                // hit_index = mask ? index : hit_index;
                hit_index = _mm_blendv_epi8(hit_index, index, _mm_castps_si128(mask));
                // hit_t = mask ? t : hit_t;
                hit_t = _mm_blendv_ps(hit_t, t, mask);
            }
            // increment indices
            index = _mm_add_epi32(index, _mm_set1_epi32(4));
        }

        // copy results out into scalar code to get return value (if any)
        let mut hit_t_array = [t_max, t_max, t_max, t_max];
        _mm_storeu_ps(hit_t_array.as_mut_ptr(), hit_t);
        let (hit_t_lane, hit_t_scalar) = hit_t_array.iter().enumerate().fold(
            (self.len, t_max),
            |result, t| if *t.1 < result.1 { (t.0, *t.1) } else { result },
        );
        // .min_by(|x, y| {
        //     // PartialOrd strikes again
        //     use std::cmp::Ordering;
        //     if x.1 < y.1 {
        //         Ordering::Less
        //     } else if x.1 > y.1 {
        //         Ordering::Greater
        //     } else {
        //         Ordering::Equal
        //     }
        // })
        // .unwrap();
        // let hit_t_scalar = *hit_t_scalar;
        if hit_t_scalar < t_max {
            let mut hit_index_array = [-1i32, -1, -1, -1];
            _mm_storeu_si128(hit_index_array.as_mut_ptr() as *mut __m128i, hit_index);
            let hit_index_scalar = hit_index_array[hit_t_lane] as usize;
            debug_assert!(hit_index_scalar < self.len);
            let point = ray.point_at_parameter(hit_t_scalar);
            let normal = vec3(
                point.x - self.centre_x.get_unchecked(hit_index_scalar),
                point.y - self.centre_y.get_unchecked(hit_index_scalar),
                point.z - self.centre_z.get_unchecked(hit_index_scalar),
            ) * *self.radius_inv.get_unchecked(hit_index_scalar);
            let material = &self.material.get_unchecked(hit_index_scalar);
            Some((RayHit { point, normal }, material))
        } else {
            None
        }
    }
}
