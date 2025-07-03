import * as THREE from "three";
import vertexShader from "./vertex.glsl";
import fragmentShader from "./fragment.glsl";

export const explodeShader = {
    uniforms: {
        iMouse:      { type: "v2", value: new THREE.Vector2(0, 0) },
        iResolution: { type: "v2", value: new THREE.Vector2(100, 100) },
        shrinkFactor: { type: "f", value: 0.98 },
        tDiffuse:    { type: "t", value: null },
    },
    vertexShader,
    fragmentShader,
};
