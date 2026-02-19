import { useEffect, useLayoutEffect, useRef } from "react";
import * as THREE from "three";

/**
 * Keeps the renderer size in sync with its parent element and forwards resize events to the sketch.
 */
export function useSketchResize(
  renderer: THREE.WebGLRenderer,
  onResize: (width: number, height: number) => void
) {
  const onResizeRef = useRef(onResize);

  useLayoutEffect(() => {
    onResizeRef.current = onResize;
  });

  useEffect(() => {
    const resize = () => {
      const canvas = renderer.domElement;
      const parent = canvas.parentElement;
      if (!parent) return;
      renderer.setSize(parent.clientWidth, parent.clientHeight);
      onResizeRef.current(canvas.width, canvas.height);
    };

    resize(); // initial
    window.addEventListener("resize", resize);

    return () => {
      window.removeEventListener("resize", resize);
    };
  }, [renderer]);
}