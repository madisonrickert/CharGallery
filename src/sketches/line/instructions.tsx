import classnames from "classnames";
import React from "react";
import "./instructions.css";

export interface InstructionsState {
    leapMotionControllerValid: boolean;
    lastRenderedFrame: number;
    globalFrame: number;
}

export class Instructions extends React.Component<object, InstructionsState> {
    state = {
        leapMotionControllerValid: false,
        globalFrame: 0,
        lastRenderedFrame: -Infinity,
    };

    public render() {
        const numSecondsToShowInstructions = 10;
        const shouldShow =
            !(this.state.globalFrame - this.state.lastRenderedFrame < 60 * numSecondsToShowInstructions) &&
            this.state.leapMotionControllerValid;

        return (
            <div className={classnames("line-instructions", { visible: shouldShow })}>
                <video autoPlay muted loop className="line-instructions-video">
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