import { Controller } from "leapjs";
import { map } from "@/common/math";
import { LeapConnectionStatus } from "@/common/leapStatus";

/**
 * Wires Leap controller connection events to a status callback.
 * Returns a cleanup function that removes all listeners.
 */
export function wireLeapConnectionEvents(
    controller: Controller,
    getCallback: () => ((status: LeapConnectionStatus) => void) | undefined,
) {
    const onConnect = () => getCallback()?.("connected");
    const onDisconnect = () => getCallback()?.("disconnected");
    const onStreamingStarted = () => getCallback()?.("streaming");
    const onStreamingStopped = () => getCallback()?.("connected");

    controller
        .on('connect', onConnect)
        .on('disconnect', onDisconnect)
        .on('streamingStarted', onStreamingStarted)
        .on('streamingStopped', onStreamingStopped);

    return () => {
        controller
            .removeListener('connect', onConnect)
            .removeListener('disconnect', onDisconnect)
            .removeListener('streamingStarted', onStreamingStarted)
            .removeListener('streamingStopped', onStreamingStopped);
    };
}

export function mapLeapToThreePosition(canvas: HTMLCanvasElement, position: number[]) {
    const range = [0.2, 0.8];
    // position[0] is left/right; left is negative, right is positive. each unit is one millimeter
    const x = map(position[0], -200, 200, canvas.width * range[0],  canvas.width * range[1]);
    // 40 is about 4cm, 1 inch, to 35cm = 13 inches above
    const y = map(position[1], 350, 40,   canvas.height * range[0], canvas.height * range[1]);
    return { x, y };
}