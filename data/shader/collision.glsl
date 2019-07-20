//!include vs:shape.inc fs:surface.inc

layout(location = 0) flat varying vec3 v_Vector;
layout(location = 1) varying vec3 v_World;
layout(location = 2) flat varying vec3 v_PolyNormal;
layout(location = 3) flat varying int v_TargetIndex;

layout(set = 0, binding = 0) uniform c_Globals {
    vec4 u_TargetScale;
    vec4 u_Penetration; // X=scale, Y=limit
};

#ifdef SHADER_VS
//imported: Polygon, get_shape_polygon

// Compute the exact collision vector instead of using the origin
// of the input polygon.
const float EXACT_VECTOR = 0.0;

layout(set = 3, binding = 0) uniform c_Locals {
    mat4 u_Model;
    vec4 u_ModelScale;
    uvec4 u_IndexOffset;
};

void main() {
    Polygon poly = get_shape_polygon();
    v_World = (u_Model * poly.vertex).xyz;

    v_Vector = mix(mat3(u_Model) * poly.origin, v_World - u_Model[3].xyz, EXACT_VECTOR);
    v_PolyNormal = poly.normal;
    v_TargetIndex = int(u_IndexOffset.x) + gl_InstanceIndex;

    vec2 pos = poly.vertex.xy * u_ModelScale.xy * u_TargetScale.xy;
    gl_Position = vec4(pos, 0.0, 1.0);
}
#endif //VS


#ifdef SHADER_FS
//imported: Surface, get_surface

// Each pixel of the collision grid corresponds to a level texel
// and contributes to the total momentum. The universal scale between
// individual impulses here and the rough overage computed by the
// original game is encoded in this constant.
const float SCALE = 0.01;

layout(set = 0, binding = 1, std430) buffer Storage {
    //Note: using `ivec4` here fails to compile on Metal:
    //> error: address of vector element requested
    int s_Data[];
};

void main() {
    Surface suf = get_surface(v_World.xy);

    // see `GET_MIDDLE_HIGHT` macro in the original
    float extra_room = suf.high_alt - suf.low_alt > 130.0 ? 110.0 : 48.0;
    float middle = suf.low_alt + extra_room;
    float depth_raw = max(0.0, suf.low_alt - v_World.z);

    if (v_World.z > middle && middle < suf.high_alt) {
        depth_raw = max(0.0, suf.high_alt - v_World.z);
        if (v_World.z - middle < depth_raw) {
            depth_raw = 0.0;
        }
    }

    float depth = SCALE * min(u_Penetration.y, u_Penetration.x * depth_raw);
    vec3 collision_vec = depth * vec3(v_Vector.y, -v_Vector.x, 1.0);

    ivec3 quantized = ivec3(collision_vec * 65536.0);
    atomicAdd(s_Data[4 * v_TargetIndex + 0], quantized.x);
    atomicAdd(s_Data[4 * v_TargetIndex + 1], quantized.y);
    atomicAdd(s_Data[4 * v_TargetIndex + 2], quantized.z);
    atomicAdd(s_Data[4 * v_TargetIndex + 3], 1);
}
#endif //FS
