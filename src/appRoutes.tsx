import { Route, Routes } from "react-router-dom";
import { FullPageSketch } from "./routes/fullPageSketch";
import { HomePage } from "./routes/homePage";

import { LineSketch, FlameSketch, Dots, Cymatics, Mito, Waves } from "./sketches";

export const AppRoutes = () => (
    <Routes>
        <Route path="/line" element={<FullPageSketch sketchClass={LineSketch} />} />
        <Route path="/flame" element={<FullPageSketch sketchClass={FlameSketch} />} />
        <Route path="/dots" element={<FullPageSketch sketchClass={Dots} />} />
        <Route path="/cymatics" element={<FullPageSketch sketchClass={Cymatics} />} />
        <Route path="/mito" element={<FullPageSketch sketchClass={Mito} />} />
        <Route path="/waves" element={<FullPageSketch sketchClass={Waves} />} />
        <Route path="/" element={<HomePage />} />
    </Routes>
);
