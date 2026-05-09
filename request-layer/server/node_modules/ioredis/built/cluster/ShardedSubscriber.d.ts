/// <reference types="node" />
import EventEmitter = require("events");
import { RedisOptions } from "./util";
import Redis from "../Redis";
import { ClusterOptions } from "./ClusterOptions";
declare const SubscriberStatus: {
    readonly IDLE: "idle";
    readonly STARTING: "starting";
    readonly CONNECTED: "connected";
    readonly STOPPING: "stopping";
    readonly ENDED: "ended";
};
declare type SubscriberStatus = typeof SubscriberStatus[keyof typeof SubscriberStatus];
export default class ShardedSubscriber {
    private readonly emitter;
    private readonly nodeKey;
    private status;
    private instance;
    private connectPromise;
    private lazyConnect;
    private readonly messageListeners;
    constructor(emitter: EventEmitter, options: RedisOptions, redisOptions?: ClusterOptions["redisOptions"]);
    start(): Promise<void>;
    stop(): void;
    isStarted(): boolean;
    get subscriberStatus(): SubscriberStatus;
    isHealthy(): boolean;
    getInstance(): Redis | null;
    getNodeKey(): string;
    isLazyConnect(): boolean;
    private onEnd;
    private onError;
    private onMoved;
    private updateStatus;
}
export {};
