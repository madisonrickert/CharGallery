import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import classnames from "classnames";

import { ISketch, SketchAudioContext, SketchConstructor, UI_EVENTS, UIEventName } from "@/sketch";
import { VolumeButton } from "@/components/volumeButton";
import { HandData, HandOverlay } from "@/components/HandOverlay";
import { ScreenSaver } from "@/components/screenSaver";
import { useSketchLifecycle } from "@/common/hooks/useSketchLifecycle";
import { useSketchAnimationLoop } from "@/common/hooks/useSketchAnimationLoop";
import { useSketchResize } from "@/common/hooks/useSketchResize";

import "./sketchComponent.scss";

const EVENT_LISTENER_OPTIONS: Partial<Record<UIEventName, AddEventListenerOptions>> = {
    touchstart: { passive: false },
    touchmove: { passive: false },
};

export interface ISketchComponentProps extends React.DOMAttributes<HTMLDivElement> {
    sketchClass: SketchConstructor;
}

function useSketchUIEvents(sketch: ISketch) {
    useEffect(() => {
        const canvas = sketch.renderer.domElement;
        canvas.setAttribute("tabindex", "1");

        const events = sketch.events;
        if (!events) return;

        const entries = Object.entries(events) as Array<[UIEventName, EventListener]>;
        entries.forEach(([eventName, callback]) => {
            if (callback) {
                const options = EVENT_LISTENER_OPTIONS[eventName];
                canvas.addEventListener(eventName, callback, options);
            }
        });

        return () => {
            (Object.keys(UI_EVENTS) as Array<keyof typeof UI_EVENTS>).forEach((eventName) => {
                const callback = events[eventName as UIEventName] as EventListener | undefined;
                if (callback) {
                    const options = EVENT_LISTENER_OPTIONS[eventName as UIEventName];
                    canvas.removeEventListener(eventName, callback, options);
                }
            });
        };
    }, [sketch]);
}

function SketchRenderer({ sketch }: { sketch: ISketch }) {
    const [, setTick] = useState(0);

    useSketchUIEvents(sketch);
    useSketchLifecycle(sketch);

    useSketchResize(sketch.renderer, (width, height) => {
        sketch.resize?.(width, height);
    });

    useSketchAnimationLoop(({ delta }) => {
        try {
            sketch.animate(delta);
        } catch (e) {
            console.error(e);
        }
        // Force re-render to update sketch.render() and sketch.elements
        setTick((t) => t + 1);
    });

    return (
        <div className="sketch-elements">
            {sketch.render?.()}
            {sketch.elements?.map((el, idx) => <el.type {...el.props} key={idx} />)}
        </div>
    );
}

export function SketchComponent({ sketchClass, ...containerProps }: ISketchComponentProps) {
    const [sketch, setSketch] = useState<ISketch | null>(null);
    const [volumeEnabled, setVolumeEnabled] = useState(() =>
        JSON.parse(window.localStorage.getItem("sketch-volumeEnabled") || "true")
    );
    const [handData, setHandData] = useState<HandData[]>([]);
    const [shouldShowScreenSaver, setShouldShowScreenSaver] = useState(false);

    const containerRef = useRef<HTMLDivElement | null>(null);
    const audioContextRef = useRef<SketchAudioContext | null>(null);
    const userVolumeRef = useRef<GainNode | null>(null);

    // Initialize sketch when container mounts
    useEffect(() => {
        const container = containerRef.current;
        if (!container) return;

        // Create audio context and gain nodes
        const audioContext = new AudioContext() as SketchAudioContext;
        audioContextRef.current = audioContext;
        THREE.AudioContext.setContext(audioContext);

        const userVolume = audioContext.createGain();
        userVolumeRef.current = userVolume;
        userVolume.gain.value = 1;
        userVolume.connect(audioContext.destination);

        const audioContextGain = audioContext.createGain();
        audioContext.gain = audioContextGain;
        audioContextGain.connect(userVolume);

        // Create renderer
        const renderer = new THREE.WebGLRenderer({
            alpha: true,
            preserveDrawingBuffer: true,
            antialias: true
        });
        container.appendChild(renderer.domElement);

        // Create sketch instance
        const sketchInstance = new sketchClass(renderer, audioContext);
        sketchInstance.updateScreenSaverCallback = setShouldShowScreenSaver;
        sketchInstance.updateHandDataCallback = setHandData;
        queueMicrotask(() => setSketch(sketchInstance));

        return () => {
            // Clear callbacks to prevent stale references
            sketchInstance.updateScreenSaverCallback = undefined;
            sketchInstance.updateHandDataCallback = undefined;

            // Clean up Three.js resources
            renderer.domElement.remove();
            renderer.dispose();

            // Clean up audio
            audioContext.close();
            audioContextRef.current = null;
            userVolumeRef.current = null;

            queueMicrotask(() => setSketch(null));
        };
    }, [sketchClass]);

    // Sync volume changes to audio context
    useEffect(() => {
        const audioContext = audioContextRef.current;
        const userVolume = userVolumeRef.current;
        if (!audioContext || !userVolume) return;

        userVolume.gain.value = volumeEnabled ? 1 : 0;
        if (volumeEnabled && audioContext.state === "suspended") {
            audioContext.resume();
        } else if (!volumeEnabled && audioContext.state === "running") {
            audioContext.suspend();
        }
    }, [volumeEnabled]);

    // Handle visibility changes (pause audio when tab is hidden)
    useEffect(() => {
        const handleVisibilityChange = () => {
            const audioContext = audioContextRef.current;
            if (!audioContext) return;

            if (document.hidden) {
                audioContext.suspend();
            } else if (volumeEnabled) {
                audioContext.resume();
            }
        };

        document.addEventListener("visibilitychange", handleVisibilityChange);
        return () => document.removeEventListener("visibilitychange", handleVisibilityChange);
    }, [volumeEnabled]);

    const handleVolumeButtonClick = () => {
        setVolumeEnabled((prev: boolean) => {
            const next = !prev;
            window.localStorage.setItem("sketch-volumeEnabled", JSON.stringify(next));
            return next;
        });
    };

    const className = classnames("sketch-component", sketch ? "success" : "loading");

    return (
        <div {...containerProps} id={sketchClass.id} className={className} ref={containerRef}>
            <div style={{ position: "relative" }}>
                {sketch && (
                    <>
                        <SketchRenderer key={sketchClass.id} sketch={sketch} />
                        <HandOverlay hands={handData} />
                    </>
                )}
            </div>
            <ScreenSaver shouldShow={shouldShowScreenSaver} />
            <VolumeButton volumeEnabled={volumeEnabled} onClick={handleVolumeButtonClick} />
        </div>
    );
}
