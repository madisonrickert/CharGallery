import { useEffect, useState } from "react";
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
    // Keep the video mounted briefly after hiding so the CSS fade-out completes
    const [mounted, setMounted] = useState(false);

    useEffect(() => {
        if (shouldShow) {
            setMounted(true);
        } else {
            const timer = setTimeout(() => setMounted(false), 600);
            return () => clearTimeout(timer);
        }
    }, [shouldShow]);

    return (
        <div className={classnames("screen-saver", { visible: shouldShow })}>
            {mounted && (
                <video autoPlay muted loop playsInline className="video">
                    <source src={screenSaverVideoMP4} type="video/mp4" />
                    <source src={screenSaverVideoWEBM} type="video/webm" />
                    Your browser does not support the video tag.
                </video>
            )}
            <img src={statueSVG} alt="Statue" className="statue graphic" />
            <img src={handSVG} alt="Left Hand" className="hand graphic" />
            <img src={handSVG} alt="Right Hand" className="hand hand-right graphic" />
        </div>
    );
}
