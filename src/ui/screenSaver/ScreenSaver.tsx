import { useEffect, useRef } from "react";
import classnames from "classnames";

import "./screenSaver.css";
import screenSaverVideoMP4 from "./screensaver_looped.mp4";
import screenSaverVideoWEBM from "./screensaver_looped.webm";
import statueSVG from "./statue.svg";
import handSVG from "./hand.svg";

export interface ScreenSaverProps {
    shouldShow: boolean;
}

export function ScreenSaver({ shouldShow }: ScreenSaverProps) {
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
            <img src={statueSVG} alt="Statue" className="statue graphic" />
            <img src={handSVG} alt="Left Hand" className="hand graphic" />
            <img src={handSVG} alt="Right Hand" className="hand hand-right graphic" />
        </div>
    );
}
