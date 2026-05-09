/// <reference types="node" />
import * as EventEmitter from "events";
import ShardedSubscriber from "./ShardedSubscriber";
import { ClusterOptions } from "./ClusterOptions";
/**
 * Redis distinguishes between "normal" and sharded PubSub. When using the normal PubSub feature,
 * exactly one subscriber exists per cluster instance because the Redis cluster bus forwards
 * messages between shards. Sharded PubSub removes this limitation by making each shard
 * responsible for its own messages.
 *
 * This class coordinates one ShardedSubscriber per master node in the cluster, providing
 * sharded PubSub support while keeping the public API backward compatible.
 */
export default class ClusterSubscriberGroup {
    private readonly subscriberGroupEmitter;
    private readonly options;
    private shardedSubscribers;
    private clusterSlots;
    private subscriberToSlotsIndex;
    private channels;
    private failedAttemptsByNode;
    private isResetting;
    private pendingReset;
    private static readonly MAX_RETRY_ATTEMPTS;
    private static readonly MAX_BACKOFF_MS;
    private static readonly BASE_BACKOFF_MS;
    /**
     * Register callbacks
     *
     * @param cluster
     */
    constructor(subscriberGroupEmitter: EventEmitter, options: ClusterOptions);
    /**
     * Get the responsible subscriber.
     *
     * @param slot
     */
    getResponsibleSubscriber(slot: number): ShardedSubscriber | undefined;
    /**
     * Adds a channel for which this subscriber group is responsible
     *
     * @param channels
     */
    addChannels(channels: (string | Buffer)[]): number;
    /**
     * Removes channels for which the subscriber group is responsible by optionally unsubscribing
     * @param channels
     */
    removeChannels(channels: (string | Buffer)[]): number;
    /**
     * Disconnect all subscribers and clear some of the internal state.
     */
    stop(): void;
    /**
     * Start all not yet started subscribers
     */
    start(): Promise<any[]>;
    /**
     * Resets the subscriber group by disconnecting all subscribers that are no longer needed and connecting new ones.
     */
    reset(clusterSlots: string[][], clusterNodes: any[]): Promise<void>;
    /**
     * Refreshes the subscriber-related slot ranges
     *
     * Returns false if no refresh was needed
     *
     * @param targetSlots
     */
    private _refreshSlots;
    /**
     * Resubscribes to the previous channels
     *
     * @private
     */
    private _resubscribe;
    /**
     * Deep equality of the cluster slots objects
     *
     * @param other
     * @private
     */
    private _slotsAreEqual;
    /**
     * Checks if any subscribers are in an unhealthy state.
     *
     * A subscriber is considered unhealthy if:
     * - It exists but is not started (failed/disconnected)
     * - It's missing entirely for a node that should have one
     *
     * @returns true if any subscribers need to be recreated
     */
    private hasUnhealthySubscribers;
    /**
     * Handles failed subscriber connections by emitting an event to refresh the slots cache
     * after a backoff period.
     *
     * @param error
     * @param nodeKey
     */
    private handleSubscriberConnectFailed;
    /**
     * Handles successful subscriber connections by resetting the failed attempts counter.
     *
     * @param nodeKey
     */
    private handleSubscriberConnectSucceeded;
    private shouldStartSubscriber;
}
