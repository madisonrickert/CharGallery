import { useRef, useState } from "react";
import { HexColorPicker, HexColorInput } from "react-colorful";
import { useSketchSettings } from "@/common/hooks/useSketchSettings";
import { GLOBAL_SETTINGS_DEFS, loadGlobalSettings, saveGlobalSetting } from "@/common/globalSettings";

import "./advancedSettingsPanel.scss";

export function DevSettingsPanel() {
    const { defs, settings, setSetting } = useSketchSettings();

    const [globalSettings, setGlobalSettings] = useState(loadGlobalSettings);

    const updateGlobalSetting = (key: keyof typeof GLOBAL_SETTINGS_DEFS, value: unknown) => {
        setGlobalSettings(prev => ({ ...prev, [key]: value } as typeof prev));
        saveGlobalSetting(key, value);
    };

    const devEntries = Object.entries(defs).filter(([, def]) => def.category === "dev");
    const globalDevEntries = Object.entries(GLOBAL_SETTINGS_DEFS).filter(([, def]) => def.category === "dev");

    return (
        <div className="overlay-panel advanced-settings-panel">
            <div className="overlay-panel-title">Advanced Settings</div>
            {globalDevEntries.map(([key, def]) => (
                <SettingRow
                    key={`global-${key}`}
                    def={def}
                    value={globalSettings[key as keyof typeof globalSettings]}
                    onChange={(value) => updateGlobalSetting(key as keyof typeof GLOBAL_SETTINGS_DEFS, value)}
                />
            ))}
            {devEntries.map(([key, def]) => (
                <SettingRow
                    key={key}
                    def={def}
                    value={settings[key]}
                    onChange={(value) => setSetting(key, value)}
                />
            ))}
        </div>
    );
}

const MAX_IMAGE_DATA_URL_SIZE = 512 * 1024; // 512KB

function SettingRow({ def, value, onChange }: {
    def: { label: string; requiresRestart?: boolean; default: unknown; step?: number; type?: "color" | "image" };
    value: unknown;
    onChange: (value: unknown) => void;
}) {
    const [colorOpen, setColorOpen] = useState(false);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const [imageError, setImageError] = useState<string | null>(null);

    const handleImageUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
        const file = e.target.files?.[0];
        if (!file) return;
        setImageError(null);

        const reader = new FileReader();
        reader.onload = () => {
            const dataUrl = reader.result as string;
            if (dataUrl.length > MAX_IMAGE_DATA_URL_SIZE) {
                setImageError(`Image too large (${Math.round(dataUrl.length / 1024)}KB). Max ${MAX_IMAGE_DATA_URL_SIZE / 1024}KB.`);
                return;
            }
            onChange(dataUrl);
        };
        reader.readAsDataURL(file);

        // Reset so the same file can be re-selected
        e.target.value = "";
    };

    return (
        <label className="overlay-panel-row advanced-settings-row">
            <span className="overlay-panel-label">
                {def.label}
                {def.requiresRestart && <span className="advanced-settings-restart"> (restart)</span>}
            </span>
            {typeof def.default === "boolean" ? (
                <button
                    type="button"
                    role="switch"
                    aria-checked={value as boolean}
                    className={`advanced-settings-toggle-switch ${value ? "on" : ""}`}
                    onClick={() => onChange(!(value as boolean))}
                >
                    <span className="advanced-settings-toggle-knob" />
                </button>
            ) : def.type === "color" ? (
                <div className="advanced-settings-color">
                    <button
                        type="button"
                        className="advanced-settings-color-swatch"
                        style={{ backgroundColor: value as string }}
                        onClick={() => setColorOpen(!colorOpen)}
                    />
                    <HexColorInput
                        className="advanced-settings-color-input"
                        color={value as string}
                        prefixed
                        onChange={(c) => onChange(c)}
                    />
                    {colorOpen && (
                        <div className="advanced-settings-color-popover">
                            <HexColorPicker color={value as string} onChange={(c) => onChange(c)} />
                        </div>
                    )}
                </div>
            ) : def.type === "image" ? (
                <div className="advanced-settings-image">
                    <input
                        ref={fileInputRef}
                        type="file"
                        accept="image/*"
                        style={{ display: "none" }}
                        onChange={handleImageUpload}
                    />
                    {(value as string) && (
                        <img
                            className="advanced-settings-image-preview"
                            src={value as string}
                            alt="Spawn template"
                        />
                    )}
                    <button
                        type="button"
                        className="advanced-settings-image-btn"
                        onClick={(e) => { e.preventDefault(); fileInputRef.current?.click(); }}
                    >
                        Upload
                    </button>
                    {(value as string) && (
                        <button
                            type="button"
                            className="advanced-settings-image-btn advanced-settings-image-reset"
                            onClick={(e) => { e.preventDefault(); onChange(""); setImageError(null); }}
                        >
                            Reset
                        </button>
                    )}
                    {imageError && <span className="advanced-settings-image-error">{imageError}</span>}
                </div>
            ) : typeof def.default === "number" ? (
                <input
                    type="number"
                    value={value as number}
                    step={def.step}
                    onChange={(e) => onChange(e.target.valueAsNumber || 0)}
                />
            ) : (
                <input
                    type="text"
                    value={value as string}
                    onChange={(e) => onChange(e.target.value)}
                />
            )}
        </label>
    );
}
