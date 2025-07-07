import * as Leap from "leapjs";
import { mapLeapToThreePosition } from "@/common/leap/util";
import { HandMesh } from "@/common/leap/handMesh";
import LineSketch from ".";

/**
 * Wrapper for the Leap Motion controller inside of the line sketch.
 */
export class LeapAttractorController {
    public controller: Leap.Controller;

    constructor(public sketch: LineSketch) {
        this.controller = new Leap.Controller().connect();
    }

    /**
     * Attach the Leap Motion controller to the sketch.
     * This will start listening for Leap Motion frames and handle them.
     */
    attachToLeap() {
        this.controller.on('frame', this.handleFrame);
    }

    /**
     * Handle a Leap Motion frame.
     * @param frame The Leap Motion frame to handle.
     */
    private handleFrame = (frame: Leap.Frame) => {
        if (frame.hands.length > 0) {
            this.sketch.lastRenderedFrame = this.sketch.globalFrame;
        }
        // Hide all hand meshes and zero all attractor powers
        for (const attractor of this.sketch.attractors) {
            if (attractor.handMesh != null) {
                attractor.handMesh.visible = false;
            }
            attractor.power = 0;
        }
        frame.hands.filter((hand) => hand.valid).forEach((hand, index) => {
            const position = hand.indexFinger.bones[3].center();
            const { x, y } = mapLeapToThreePosition(this.sketch.canvas, position);
            this.sketch.setMousePosition(x, y);

            // Use lazy attractor pool
            const attractor = this.sketch.getAttractor(index);
            attractor.x = x;
            attractor.y = y;

            if (hand.indexFinger.extended) {
                attractor.power = attractor.power * 0.5;
            } else {
                // position[2] goes from -300 to 300
                const wantedPower = Math.pow(7, (-position[2] + 350) / 200);
                attractor.power = attractor.power * 0.5 + wantedPower * 0.5;
            }

            if (attractor.handMesh == null) {
                attractor.handMesh = new HandMesh();
                this.sketch.scene.add(attractor.handMesh);
            }
            attractor.handMesh.update(this.sketch.canvas, hand);
            attractor.handMesh!.visible = true;
        });
    }

    /**
     * Detach the Leap Motion controller from the sketch.
     * This will stop listening for Leap Motion frames.
     */
    detachFromLeap() {
        this.controller.removeListener('frame', this.handleFrame);
    }

    lastFrameIsValid() {
        return this.controller.lastFrame.valid;
    }
}
