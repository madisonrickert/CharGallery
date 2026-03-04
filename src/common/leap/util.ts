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

const LEAP_RANGE_MIN = 0.2;
const LEAP_RANGE_MAX = 0.8;

export function mapLeapToThreePosition(canvas: HTMLCanvasElement, position: number[]) {
    // position[0] is left/right; left is negative, right is positive. each unit is one millimeter
    const x = map(position[0], -200, 200, canvas.width * LEAP_RANGE_MIN,  canvas.width * LEAP_RANGE_MAX);
    // 40 is about 4cm, 1 inch, to 35cm = 13 inches above
    const y = map(position[1], 350, 40,   canvas.height * LEAP_RANGE_MIN, canvas.height * LEAP_RANGE_MAX);
    return { x, y };
}