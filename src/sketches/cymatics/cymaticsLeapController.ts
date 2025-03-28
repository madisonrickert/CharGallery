import * as Leap from 'leapjs';
import { Cymatics } from './cymatics';
import { mapLeapToThreePosition } from "../../common/leap/util";
// import { HandMesh } from "../../common/leap/handMesh";

export function initLeap(sketch: Cymatics, onLoopPinched: () => void, onLoopUnpinched: () => void): Leap.Controller {
    // const handMeshes: Array<HandMesh> = [];
    const controller = Leap.loop((frame: Leap.Frame) => {
        if (frame.hands.length > 0) {
            // @todo Break this out so that it's not dependent just on leap interaction but on all user input
            // sketch.lastRenderedFrame = sketch.globalFrame;
        }
         frame.hands.filter((hand) => hand.valid).forEach((hand, _index) => {
            const position = hand.indexFinger.bones[3].center();

            const {x, y} = mapLeapToThreePosition(sketch.canvas, position);
            sketch.setMouse(x, y);

            if (hand.indexFinger.extended) {
                onLoopUnpinched();
            } else {
                onLoopPinched();
            }

            // if (handMeshes[index] == null) {
            //     handMeshes[index] = new HandMesh();
            //     sketch.handScene.add(handMeshes[index]);
            // }
            // handMeshes[index].update(sketch.canvas, frame.hands[0]);
            // handMeshes[index]!.visible = true;
         });
    });
    return controller;
}