import React from "react";

import { SketchConstructor } from "../sketch";
import { SketchComponent } from "../sketchComponent";

import "./fullPageSketch.css";

export interface ISketchRouteProps {
    sketchClass: SketchConstructor;
}

export class FullPageSketch extends React.Component<ISketchRouteProps, object> {
    public render() {
        return (
            <div className={"full-page-sketch"}>
                <SketchComponent sketchClass={this.props.sketchClass} />
            </div>
        );
    }
}
