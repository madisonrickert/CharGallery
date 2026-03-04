import { LeapProcessStatus } from "@/common/leapStatus";

interface ElectronAPI {
  getLeapProcessStatus: () => Promise<LeapProcessStatus>;
  startLeapProcess: () => Promise<LeapProcessStatus>;
  stopLeapProcess: () => Promise<LeapProcessStatus>;
  onLeapProcessStatus: (callback: (status: LeapProcessStatus) => void) => () => void;
}

interface Window {
  electronAPI?: ElectronAPI;
}
