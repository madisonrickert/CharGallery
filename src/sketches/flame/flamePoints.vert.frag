uniform float size;
uniform float scale;
uniform float focalLength;
varying float opacityScalar;

#include <common>
#include <color_pars_vertex>
#include <fog_pars_vertex>
#include <morphtarget_pars_vertex>
#include <logdepthbuf_pars_vertex>
#include <clipping_planes_pars_vertex>

void main() {

	#include <color_vertex>
	#include <begin_vertex>
	#include <morphtarget_vertex>
	#include <project_vertex>

    float dist = -mvPosition.z;
    float originalSize = 2.0;
    float outOfFocusAmount = pow(abs(dist - focalLength) / focalLength, 2.) * 3.;
    float unfocusedSize = originalSize * (1. + outOfFocusAmount);

    gl_PointSize = unfocusedSize * ( scale / - mvPosition.z );
    gl_PointSize = min(50., gl_PointSize);

    opacityScalar = 1. / pow(1. + outOfFocusAmount, 2.);

	#include <logdepthbuf_vertex>
	#include <clipping_planes_vertex>
	#include <worldpos_vertex>
	#include <fog_vertex>

}
