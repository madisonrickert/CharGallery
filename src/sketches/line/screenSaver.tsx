import classnames from "classnames";
import React from "react";
import "./screenSaver.css";

export interface ScreenSaverState {
    leapMotionControllerValid: boolean;
    lastRenderedFrame: number;
    globalFrame: number;
}

export class ScreenSaver extends React.Component<object, ScreenSaverState> {
    state = {
        leapMotionControllerValid: false,
        globalFrame: 0,
        lastRenderedFrame: -Infinity,
    };

    public render() {
        const numSecondsToShowScreenSaver = 10;
        const shouldShow =
            !(this.state.globalFrame - this.state.lastRenderedFrame < 60 * numSecondsToShowScreenSaver) &&
            this.state.leapMotionControllerValid;

        return (
            <div className={classnames("screen-saver", { visible: shouldShow })}>
                <video autoPlay muted loop className="screen-saver-video">
                    <source src="/assets/images/capture.mp4" type="video/mp4" />
                    Your browser does not support the video tag.
                </video>
            </div>
        );
    }

    public setGlobalFrame(f: number) {
        this.setState({ globalFrame: f });
    }

    public setLastRenderedFrame(lastRenderedFrame: number) {
        console.log(lastRenderedFrame);
        this.setState({ lastRenderedFrame });
    }

    public setLeapMotionControllerValid(valid: boolean) {
        this.setState({ leapMotionControllerValid: valid });
    }
}