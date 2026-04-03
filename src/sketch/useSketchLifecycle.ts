import { useEffect } from "react";
import { BaseSketch } from "@/sketch/BaseSketch";

/**
 * Runs init/cleanup for the sketch.
 */
export function useSketchLifecycle(sketch: BaseSketch) {
  useEffect(() => {
    sketch.init();

    return () => {
      sketch.destroy();
    };
  }, [sketch]);
}