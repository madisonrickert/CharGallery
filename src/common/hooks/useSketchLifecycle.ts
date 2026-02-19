import { useEffect } from "react";
import { ISketch } from "@/sketch";

/**
 * Runs init/cleanup for the sketch.
 */
export function useSketchLifecycle(sketch: ISketch) {
  useEffect(() => {
    sketch.init();

    return () => {
      sketch.destroy?.();
    };
  }, [sketch]);
}