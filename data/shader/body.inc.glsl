struct Data {
    vec4 pos_scale;
    vec4 orientation;
    vec4 linear;
    vec4 angular;
    vec4 springs;
    vec4 scale_volume_zomc; // X=shape scale, Y=volume, Z = Z offset of mass center
    mat4 jacobian_inv;
};
