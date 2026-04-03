import { useCallback, useEffect, useState } from "react";

import { useAudioContext } from "@/audio/useAudioContext";
import { loadGlobalSettings, saveGlobalSetting } from "@/settings/globalSettings";

/**
 * Manages the user-facing volume toggle: persists preference to localStorage,
 * syncs gain to the shared AudioContext, and suspends/resumes audio on tab
 * visibility changes to save resources when the tab is hidden.
 */
export function useVolume() {
    const { setUserVolume } = useAudioContext();
    const [volumeEnabled, setVolumeEnabled] = useState(() => loadGlobalSettings().volumeEnabled);

    const toggleVolume = useCallback(() => {
        setVolumeEnabled((prev: boolean) => {
            const next = !prev;
            saveGlobalSetting("volumeEnabled", next);
            return next;
        });
    }, []);

    // Sync volume state to the AudioContext gain node
    useEffect(() => {
        setUserVolume(volumeEnabled ? 1 : 0);
    }, [volumeEnabled, setUserVolume]);

    // Pause audio when the browser tab is hidden or phone is locked.
    // visibilitychange covers tab switches; pagehide/pageshow and blur/focus
    // improve reliability on iOS lock screen.
    useEffect(() => {
        const mute = () => setUserVolume(0);
        const restore = () => setUserVolume(volumeEnabled ? 1 : 0);
        const handleVisibilityChange = () => {
            if (document.hidden) mute(); else restore();
        };

        document.addEventListener("visibilitychange", handleVisibilityChange);
        window.addEventListener("pagehide", mute);
        window.addEventListener("pageshow", restore);
        window.addEventListener("blur", mute);
        window.addEventListener("focus", restore);
        return () => {
            document.removeEventListener("visibilitychange", handleVisibilityChange);
            window.removeEventListener("pagehide", mute);
            window.removeEventListener("pageshow", restore);
            window.removeEventListener("blur", mute);
            window.removeEventListener("focus", restore);
        };
    }, [volumeEnabled, setUserVolume]);

    return { volumeEnabled, toggleVolume };
}
