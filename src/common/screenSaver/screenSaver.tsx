import classnames from "classnames";
import React from "react";
import "./screenSaver.css";

export interface ScreenSaverProps {
    shouldShow: boolean;
}

export class ScreenSaver extends React.Component<ScreenSaverProps> {
    public render() {
        return (
            <div className={classnames("screen-saver", { visible: this.props.shouldShow })}>
                <video autoPlay muted loop className="screen-saver-video">
                    <source src="/assets/images/capture.mp4" type="video/mp4" />
                    Your browser does not support the video tag.
                </video>
            </div>
        );
    }
}