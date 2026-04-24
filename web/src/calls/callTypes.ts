export type CallSignalType = "offer" | "answer" | "ice";

export interface CallMediaState {
  audio: boolean;
  camera: boolean;
  screen: boolean;
}

export interface CallSignalPayload {
  gameId: string;
  to: string;
  from?: string;
  type: CallSignalType;
  sdp?: string;
  candidate?: RTCIceCandidateInit;
}

export interface RemoteCallMedia {
  stream: MediaStream;
  media: CallMediaState;
}

export const EMPTY_CALL_MEDIA: CallMediaState = {
  audio: false,
  camera: false,
  screen: false
};

export function normalizeCallMedia(value: Partial<CallMediaState> | null | undefined): CallMediaState {
  return {
    audio: Boolean(value?.audio),
    camera: Boolean(value?.camera),
    screen: Boolean(value?.screen)
  };
}
