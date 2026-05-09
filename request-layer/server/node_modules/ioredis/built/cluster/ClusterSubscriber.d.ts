/// <reference types="node" />
import { EventEmitter } from "events";
import ConnectionPool from "./ConnectionPool";
export default class ClusterSubscriber {
    private connectionPool;
    private emitter;
    private isSharded;
    private started;
    private subscriber;
    private lastActiveSubscriber;
    private slotRange;
    constructor(connectionPool: ConnectionPool, emitter: EventEmitter, isSharded?: boolean);
    getInstance(): any;
    /**
     * Associate this subscriber to a specific slot range.
     *
     * Returns the range or an empty array if the slot range couldn't be associated.
     *
     * BTW: This is more for debugging and testing purposes.
     *
     * @param range
     */
    associateSlotRange(range: number[]): number[];
    start(): void;
    stop(): void;
    isStarted(): boolean;
    private onSubscriberEnd;
    private selectSubscriber;
}
