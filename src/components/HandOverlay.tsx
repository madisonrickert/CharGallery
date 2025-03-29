import React from "react";
import { FaHandPaper, FaHandRock } from "react-icons/fa";

export interface HandData {
  index: number;
  position: { x: number; y: number };
  pinched: boolean;
}

interface HandOverlayProps {
  hands: HandData[];
}

export const HandOverlay: React.FC<HandOverlayProps> = ({ hands }) => {
  return (
    <div 
      className="hand-overlay" 
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        pointerEvents: "none",
        zIndex: 10,
      }}
    >
      {hands.map((hand) => (
        <div
          key={hand.index}
          style={{
            position: "absolute",
            left: `${hand.position.x}px`,
            top: `${hand.position.y}px`,
            transform: "translate(-50%, -50%)",
            fontSize: "32px",
            color: "rgba(255, 255, 255, 0.5)",
          }}
        >
          {hand.pinched ? <FaHandRock /> : <FaHandPaper />}
        </div>
      ))}
    </div>
  );
};
