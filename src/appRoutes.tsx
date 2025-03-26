import { Route, Routes } from "react-router-dom";
import { HomePage } from "./routes/homePage";
import { SketchComponent } from "./sketchComponent";

import { LineSketch, FlameSketch, Dots, Cymatics, Mito, Waves } from "./sketches";

export const AppRoutes = () => (
    <Routes>
        <Route path="/line" element={<SketchComponent sketchClass={LineSketch} />} />
        <Route path="/flame" element={<SketchComponent sketchClass={FlameSketch} />} />
        <Route path="/dots" element={<SketchComponent sketchClass={Dots} />} />
        <Route path="/cymatics" element={<SketchComponent sketchClass={Cymatics} />} />
        <Route path="/mito" element={<SketchComponent sketchClass={Mito} />} />
        <Route path="/waves" element={<SketchComponent sketchClass={Waves} />} />
        <Route path="/" element={<HomePage />} />
    </Routes>
);
