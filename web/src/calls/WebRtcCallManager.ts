import type { CallMediaState, CallSignalPayload, RemoteCallMedia } from "./callTypes";
import { rtcRuntimeConfig } from "@/config/runtime";

interface PeerState {
  pc: RTCPeerConnection;
  stream: MediaStream;
  makingOffer: boolean;
  ignoreOffer: boolean;
  settingRemoteAnswer: boolean;
  pendingCandidates: RTCIceCandidateInit[];
  signalChain: Promise<void>;
}

interface WebRtcCallManagerOptions {
  roomId: string;
  username: string;
  localStream: MediaStream;
  sendSignal: (payload: CallSignalPayload) => void;
  onRemoteMedia: (username: string, media: RemoteCallMedia) => void;
  onRemoteLeft: (username: string) => void;
}

const relayUrls = rtcRuntimeConfig.turnUrls;
const relayUsername = rtcRuntimeConfig.turnUsername;
const relayCredential = rtcRuntimeConfig.turnCredential;

export function relayCallsConfigured() {
  return rtcRuntimeConfig.callsEnabled && relayUrls.length > 0 && !!relayUsername && !!relayCredential;
}

export function relayCallsRequirementMessage() {
  return rtcRuntimeConfig.callsUnavailableReason
    || "Calls are disabled until a TURN relay is configured. Direct peer-to-peer calls were turned off to avoid exposing participant IP addresses.";
}

const rtcConfig: RTCConfiguration = relayCallsConfigured()
  ? {
      iceTransportPolicy: "relay",
      iceServers: [{
        urls: relayUrls,
        username: relayUsername,
        credential: relayCredential
      }]
    }
  : {
      iceTransportPolicy: "relay",
      iceServers: []
    };

export class WebRtcCallManager {
  private readonly peers = new Map<string, PeerState>();
  private readonly roomId: string;
  private readonly username: string;
  private readonly sendSignal: WebRtcCallManagerOptions["sendSignal"];
  private readonly onRemoteMedia: WebRtcCallManagerOptions["onRemoteMedia"];
  private readonly onRemoteLeft: WebRtcCallManagerOptions["onRemoteLeft"];
  private localStream: MediaStream;
  private localMedia: CallMediaState = { audio: true, camera: false, screen: false };

  constructor(options: WebRtcCallManagerOptions) {
    this.roomId = options.roomId;
    this.username = options.username;
    this.localStream = options.localStream;
    this.sendSignal = options.sendSignal;
    this.onRemoteMedia = options.onRemoteMedia;
    this.onRemoteLeft = options.onRemoteLeft;
  }

  connectPeer(peerName: string) {
    if (!peerName || peerName === this.username || this.peers.has(peerName)) return;
    this.createPeer(peerName);
  }

  removePeer(peerName: string) {
    const peer = this.peers.get(peerName);
    if (!peer) return;
    peer.pc.close();
    for (const track of peer.stream.getTracks()) track.stop();
    this.peers.delete(peerName);
    this.onRemoteLeft(peerName);
  }

  setLocalMedia(media: CallMediaState) {
    this.localMedia = { ...media };
  }

  addLocalStream(stream: MediaStream) {
    for (const track of stream.getTracks()) {
      this.localStream.addTrack(track);
      for (const peer of this.peers.values()) {
        peer.pc.addTrack(track, this.localStream);
      }
    }
  }

  removeLocalTracks(predicate: (track: MediaStreamTrack) => boolean) {
    const tracks = this.localStream.getTracks().filter(predicate);
    for (const track of tracks) {
      for (const peer of this.peers.values()) {
        const sender = peer.pc.getSenders().find((item) => item.track === track);
        if (sender) peer.pc.removeTrack(sender);
      }
      this.localStream.removeTrack(track);
      track.stop();
    }
  }

  async handleSignal(payload: CallSignalPayload) {
    const from = payload.from;
    if (!from || from === this.username || payload.gameId !== this.roomId) return;

    const peer = this.peers.get(from) || this.createPeer(from);
    peer.signalChain = peer.signalChain
      .then(() => this.applySignal(from, peer, payload))
      .catch(() => this.removePeer(from));
    await peer.signalChain;
  }

  private async applySignal(from: string, peer: PeerState, payload: CallSignalPayload) {
    if (this.peers.get(from) !== peer) return;

    const description = payload.sdp
      ? ({ type: payload.type, sdp: payload.sdp } as RTCSessionDescriptionInit)
      : null;

    if (description) {
      const readyForOffer =
        !peer.makingOffer &&
        (peer.pc.signalingState === "stable" || peer.settingRemoteAnswer);
      const offerCollision = description.type === "offer" && !readyForOffer;
      peer.ignoreOffer = !this.isPolite(from) && offerCollision;
      if (peer.ignoreOffer) return;

      peer.settingRemoteAnswer = description.type === "answer";
      try {
        await peer.pc.setRemoteDescription(description);
      } finally {
        peer.settingRemoteAnswer = false;
      }

      await this.flushPendingCandidates(peer);
      if (description.type === "offer") {
        await peer.pc.setLocalDescription();
        this.sendSignal({
          gameId: this.roomId,
          to: from,
          type: "answer",
          sdp: peer.pc.localDescription?.sdp || ""
        });
      }
    } else if (payload.candidate) {
      if (!peer.pc.remoteDescription) {
        if (peer.ignoreOffer) return;
        peer.pendingCandidates.push(payload.candidate);
        return;
      }
      await this.addIceCandidate(peer, payload.candidate);
    }
  }

  close() {
    for (const peerName of [...this.peers.keys()]) this.removePeer(peerName);
  }

  private createPeer(peerName: string): PeerState {
    const pc = new RTCPeerConnection(rtcConfig);
    const stream = new MediaStream();
    const peer: PeerState = {
      pc,
      stream,
      makingOffer: false,
      ignoreOffer: false,
      settingRemoteAnswer: false,
      pendingCandidates: [],
      signalChain: Promise.resolve()
    };
    this.peers.set(peerName, peer);

    pc.onicecandidate = ({ candidate }) => {
      if (!candidate) return;
      this.sendSignal({
        gameId: this.roomId,
        to: peerName,
        type: "ice",
        candidate: candidate.toJSON()
      });
    };

    pc.ontrack = ({ track, streams }) => {
      const source = streams[0];
      if (source) {
        for (const item of source.getTracks()) {
          if (!stream.getTracks().some((existing) => existing.id === item.id)) stream.addTrack(item);
        }
      } else if (!stream.getTracks().some((existing) => existing.id === track.id)) {
        stream.addTrack(track);
      }
      track.onended = () => this.onRemoteMedia(peerName, { stream, media: this.inferRemoteMedia(stream) });
      this.onRemoteMedia(peerName, { stream, media: this.inferRemoteMedia(stream) });
    };

    pc.onconnectionstatechange = () => {
      if (["closed", "failed"].includes(pc.connectionState)) {
        this.removePeer(peerName);
      }
    };

    pc.onnegotiationneeded = async () => {
      try {
        peer.makingOffer = true;
        await pc.setLocalDescription();
        this.sendSignal({
          gameId: this.roomId,
          to: peerName,
          type: "offer",
          sdp: pc.localDescription?.sdp || ""
        });
      } catch {
        this.removePeer(peerName);
      } finally {
        peer.makingOffer = false;
      }
    };

    for (const track of this.localStream.getTracks()) {
      pc.addTrack(track, this.localStream);
    }

    return peer;
  }

  private async addIceCandidate(peer: PeerState, candidate: RTCIceCandidateInit) {
    try {
      await peer.pc.addIceCandidate(candidate);
    } catch (error) {
      if (!peer.ignoreOffer) throw error;
    }
  }

  private async flushPendingCandidates(peer: PeerState) {
    const candidates = peer.pendingCandidates.splice(0);
    for (const candidate of candidates) {
      await this.addIceCandidate(peer, candidate);
    }
  }

  private isPolite(peerName: string) {
    return this.username.localeCompare(peerName) > 0;
  }

  private inferRemoteMedia(stream: MediaStream): CallMediaState {
    return {
      audio: stream.getAudioTracks().some((track) => track.readyState === "live"),
      camera: stream.getVideoTracks().some((track) => track.readyState === "live"),
      screen: false
    };
  }
}
