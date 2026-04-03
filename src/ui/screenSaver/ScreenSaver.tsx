import { useEffect, useRef } from "react";
import classnames from "classnames";

import "./screenSaver.css";
import screenSaverVideoMP4 from "./screensaver_looped.mp4";
import screenSaverVideoWEBM from "./screensaver_looped.webm";
import statueSVG from "./statue.svg";
import handSVG from "./hand.svg";
import handPointSVG from "./hand-point.svg";

export type DismissMethod = "mouse" | "touch" | "motion";

export interface ScreenSaverProps {
    shouldShow: boolean;
    dismissMethod: DismissMethod;
}

export function ScreenSaver({ shouldShow, dismissMethod }: ScreenSaverProps) {
    const videoRef = useRef<HTMLVideoElement>(null);

    // Control video playback imperatively to avoid iOS Safari's fullscreen
    // takeover of autoplaying videos. The video stays in the DOM but only
    // plays when the screensaver is visible.
    useEffect(() => {
        const video = videoRef.current;
        if (!video) return;
        if (shouldShow) {
            video.play().catch(() => {});
        } else {
            video.pause();
            video.currentTime = 0;
        }
    }, [shouldShow]);

    return (
        <div className={classnames("screen-saver", { visible: shouldShow })}>
            <video ref={videoRef} muted loop playsInline className="video">
                <source src={screenSaverVideoMP4} type="video/mp4" />
                <source src={screenSaverVideoWEBM} type="video/webm" />
                Your browser does not support the video tag.
            </video>
            {dismissMethod === "motion" ? (
                <>
                    <img src={statueSVG} alt="Statue" className="statue graphic" />
                    <img src={handSVG} alt="Left Hand" className="hand graphic" />
                    <img src={handSVG} alt="Right Hand" className="hand hand-right graphic" />
                </>
            ) : (
                <img src={handPointSVG} alt="Touch" className="hand-point graphic" />
            )}
        </div>
    );
}
