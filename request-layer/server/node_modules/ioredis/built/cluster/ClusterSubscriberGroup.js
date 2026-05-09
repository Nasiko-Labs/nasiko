"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const utils_1 = require("../utils");
const util_1 = require("./util");
const calculateSlot = require("cluster-key-slot");
const ShardedSubscriber_1 = require("./ShardedSubscriber");
const debug = (0, utils_1.Debug)("cluster:subscriberGroup");
/**
 * Redis distinguishes between "normal" and sharded PubSub. When using the normal PubSub feature,
 * exactly one subscriber exists per cluster instance because the Redis cluster bus forwards
 * messages between shards. Sharded PubSub removes this limitation by making each shard
 * responsible for its own messages.
 *
 * This class coordinates one ShardedSubscriber per master node in the cluster, providing
 * sharded PubSub support while keeping the public API backward compatible.
 */
class ClusterSubscriberGroup {
    /**
     * Register callbacks
     *
     * @param cluster
     */
    constructor(subscriberGroupEmitter, options) {
        this.subscriberGroupEmitter = subscriberGroupEmitter;
        this.options = options;
        this.shardedSubscribers = new Map();
        this.clusterSlots = [];
        // Simple [min, max] slot ranges aren't enough because you can migrate single slots
        this.subscriberToSlotsIndex = new Map();
        this.channels = new Map();
        this.failedAttemptsByNode = new Map();
        // Only latest pending reset kept; throttled by refreshSlotsCache's isRefreshing + backoff delay
        this.isResetting = false;
        this.pendingReset = null;
        /**
         * Handles failed subscriber connections by emitting an event to refresh the slots cache
         * after a backoff period.
         *
         * @param error
         * @param nodeKey
         */
        this.handleSubscriberConnectFailed = (error, nodeKey) => {
            const currentAttempts = this.failedAttemptsByNode.get(nodeKey) || 0;
            const failedAttempts = currentAttempts + 1;
            this.failedAttemptsByNode.set(nodeKey, failedAttempts);
            const attempts = Math.min(failedAttempts, ClusterSubscriberGroup.MAX_RETRY_ATTEMPTS);
            const backoff = Math.min(ClusterSubscriberGroup.BASE_BACKOFF_MS * 2 ** attempts, ClusterSubscriberGroup.MAX_BACKOFF_MS);
            const jitter = Math.floor((Math.random() - 0.5) * (backoff * 0.5));
            const delay = Math.max(0, backoff + jitter);
            debug("Failed to connect subscriber for %s. Refreshing slots in %dms", nodeKey, delay);
            this.subscriberGroupEmitter.emit("subscriberConnectFailed", {
                delay,
                error,
            });
        };
        /**
         * Handles successful subscriber connections by resetting the failed attempts counter.
         *
         * @param nodeKey
         */
        this.handleSubscriberConnectSucceeded = (nodeKey) => {
            this.failedAttemptsByNode.delete(nodeKey);
        };
    }
    /**
     * Get the responsible subscriber.
     *
     * @param slot
     */
    getResponsibleSubscriber(slot) {
        const nodeKey = this.clusterSlots[slot][0];
        const sub = this.shardedSubscribers.get(nodeKey);
        if (sub && sub.subscriberStatus === "idle") {
            sub
                .start()
                .then(() => {
                this.handleSubscriberConnectSucceeded(sub.getNodeKey());
            })
                .catch((err) => {
                this.handleSubscriberConnectFailed(err, sub.getNodeKey());
            });
        }
        return sub;
    }
    /**
     * Adds a channel for which this subscriber group is responsible
     *
     * @param channels
     */
    addChannels(channels) {
        const slot = calculateSlot(channels[0]);
        // Check if the all channels belong to the same slot and otherwise reject the operation
        for (const c of channels) {
            if (calculateSlot(c) !== slot) {
                return -1;
            }
        }
        const currChannels = this.channels.get(slot);
        if (!currChannels) {
            this.channels.set(slot, channels);
        }
        else {
            this.channels.set(slot, currChannels.concat(channels));
        }
        return Array.from(this.channels.values()).reduce((sum, array) => sum + array.length, 0);
    }
    /**
     * Removes channels for which the subscriber group is responsible by optionally unsubscribing
     * @param channels
     */
    removeChannels(channels) {
        const slot = calculateSlot(channels[0]);
        // Check if the all channels belong to the same slot and otherwise reject the operation
        for (const c of channels) {
            if (calculateSlot(c) !== slot) {
                return -1;
            }
        }
        const slotChannels = this.channels.get(slot);
        if (slotChannels) {
            const updatedChannels = slotChannels.filter((c) => !channels.includes(c));
            this.channels.set(slot, updatedChannels);
        }
        return Array.from(this.channels.values()).reduce((sum, array) => sum + array.length, 0);
    }
    /**
     * Disconnect all subscribers and clear some of the internal state.
     */
    stop() {
        for (const s of this.shardedSubscribers.values()) {
            s.stop();
        }
        // Clear subscriber instances and pending operations.
        // Channels are preserved for resubscription on reconnect.
        this.pendingReset = null;
        this.shardedSubscribers.clear();
        this.subscriberToSlotsIndex.clear();
    }
    /**
     * Start all not yet started subscribers
     */
    start() {
        const startPromises = [];
        for (const s of this.shardedSubscribers.values()) {
            if (this.shouldStartSubscriber(s)) {
                startPromises.push(s
                    .start()
                    .then(() => {
                    this.handleSubscriberConnectSucceeded(s.getNodeKey());
                })
                    .catch((err) => {
                    this.handleSubscriberConnectFailed(err, s.getNodeKey());
                }));
                this.subscriberGroupEmitter.emit("+subscriber");
            }
        }
        return Promise.all(startPromises);
    }
    /**
     * Resets the subscriber group by disconnecting all subscribers that are no longer needed and connecting new ones.
     */
    async reset(clusterSlots, clusterNodes) {
        if (this.isResetting) {
            this.pendingReset = { slots: clusterSlots, nodes: clusterNodes };
            return;
        }
        this.isResetting = true;
        try {
            const hasTopologyChanged = this._refreshSlots(clusterSlots);
            const hasFailedSubscribers = this.hasUnhealthySubscribers();
            if (!hasTopologyChanged && !hasFailedSubscribers) {
                debug("No topology change detected or failed subscribers. Skipping reset.");
                return;
            }
            // For each of the sharded subscribers
            for (const [nodeKey, shardedSubscriber] of this.shardedSubscribers) {
                if (
                // If the subscriber is still responsible for a slot range and is healthy then keep it
                this.subscriberToSlotsIndex.has(nodeKey) &&
                    shardedSubscriber.isHealthy()) {
                    debug("Skipping deleting subscriber for %s", nodeKey);
                    continue;
                }
                debug("Removing subscriber for %s", nodeKey);
                // Otherwise stop the subscriber and remove it
                shardedSubscriber.stop();
                this.shardedSubscribers.delete(nodeKey);
                this.subscriberGroupEmitter.emit("-subscriber");
            }
            const startPromises = [];
            // For each node in slots cache
            for (const [nodeKey, _] of this.subscriberToSlotsIndex) {
                const existingSubscriber = this.shardedSubscribers.get(nodeKey);
                // If we already have a subscriber for this node, only ensure it is healthy
                // when it now owns slots with active channel subscriptions.
                if (existingSubscriber && existingSubscriber.isHealthy()) {
                    debug("Skipping creating new subscriber for %s", nodeKey);
                    if (!existingSubscriber.isStarted() &&
                        this.shouldStartSubscriber(existingSubscriber)) {
                        startPromises.push(existingSubscriber
                            .start()
                            .then(() => {
                            this.handleSubscriberConnectSucceeded(nodeKey);
                        })
                            .catch((error) => {
                            this.handleSubscriberConnectFailed(error, nodeKey);
                        }));
                    }
                    continue;
                }
                // If we have an existing subscriber but it is not healthy, stop it
                if (existingSubscriber && !existingSubscriber.isHealthy()) {
                    debug("Replacing subscriber for %s", nodeKey);
                    existingSubscriber.stop();
                    this.shardedSubscribers.delete(nodeKey);
                    this.subscriberGroupEmitter.emit("-subscriber");
                }
                debug("Creating new subscriber for %s", nodeKey);
                // Otherwise create a new subscriber
                const redis = clusterNodes.find((node) => {
                    return (0, util_1.getNodeKey)(node.options) === nodeKey;
                });
                if (!redis) {
                    debug("Failed to find node for key %s", nodeKey);
                    continue;
                }
                const sub = new ShardedSubscriber_1.default(this.subscriberGroupEmitter, redis.options, this.options.redisOptions);
                this.shardedSubscribers.set(nodeKey, sub);
                if (this.shouldStartSubscriber(sub)) {
                    startPromises.push(sub
                        .start()
                        .then(() => {
                        this.handleSubscriberConnectSucceeded(nodeKey);
                    })
                        .catch((error) => {
                        this.handleSubscriberConnectFailed(error, nodeKey);
                    }));
                }
                this.subscriberGroupEmitter.emit("+subscriber");
            }
            // It's vital to await the start promises before resubscribing
            // Otherwise we might try to resubscribe to a subscriber that is not yet connected
            // This can cause a race condition
            await Promise.all(startPromises);
            this._resubscribe();
            this.subscriberGroupEmitter.emit("subscribersReady");
        }
        finally {
            this.isResetting = false;
            if (this.pendingReset) {
                const { slots, nodes } = this.pendingReset;
                this.pendingReset = null;
                await this.reset(slots, nodes);
            }
        }
    }
    /**
     * Refreshes the subscriber-related slot ranges
     *
     * Returns false if no refresh was needed
     *
     * @param targetSlots
     */
    _refreshSlots(targetSlots) {
        //If there was an actual change, then reassign the slot ranges
        // Also rebuild if subscriberToSlotsIndex is empty (e.g., after stop() was called)
        if (this._slotsAreEqual(targetSlots) && this.subscriberToSlotsIndex.size > 0) {
            debug("Nothing to refresh because the new cluster map is equal to the previous one.");
            return false;
        }
        debug("Refreshing the slots of the subscriber group.");
        //Rebuild the slots index
        this.subscriberToSlotsIndex = new Map();
        for (let slot = 0; slot < targetSlots.length; slot++) {
            const node = targetSlots[slot][0];
            if (!this.subscriberToSlotsIndex.has(node)) {
                this.subscriberToSlotsIndex.set(node, []);
            }
            this.subscriberToSlotsIndex.get(node).push(Number(slot));
        }
        //Update the cached slots map
        this.clusterSlots = JSON.parse(JSON.stringify(targetSlots));
        return true;
    }
    /**
     * Resubscribes to the previous channels
     *
     * @private
     */
    _resubscribe() {
        if (this.shardedSubscribers) {
            this.shardedSubscribers.forEach((s, nodeKey) => {
                const subscriberSlots = this.subscriberToSlotsIndex.get(nodeKey);
                if (subscriberSlots) {
                    //Resubscribe on the underlying connection
                    subscriberSlots.forEach((ss) => {
                        //Might return null if being disconnected
                        const redis = s.getInstance();
                        const channels = this.channels.get(ss);
                        if (channels && channels.length > 0) {
                            if (!redis || redis.status === "end") {
                                return;
                            }
                            if (redis.status === "ready") {
                                redis.ssubscribe(...channels).catch((err) => {
                                    // TODO: Should we emit an error event here?
                                    debug("Failed to ssubscribe on node %s: %s", nodeKey, err);
                                });
                            }
                            else {
                                redis.once("ready", () => {
                                    redis.ssubscribe(...channels).catch((err) => {
                                        // TODO: Should we emit an error event here?
                                        debug("Failed to ssubscribe on node %s: %s", nodeKey, err);
                                    });
                                });
                            }
                        }
                    });
                }
            });
        }
    }
    /**
     * Deep equality of the cluster slots objects
     *
     * @param other
     * @private
     */
    _slotsAreEqual(other) {
        if (this.clusterSlots === undefined) {
            return false;
        }
        else {
            return JSON.stringify(this.clusterSlots) === JSON.stringify(other);
        }
    }
    /**
     * Checks if any subscribers are in an unhealthy state.
     *
     * A subscriber is considered unhealthy if:
     * - It exists but is not started (failed/disconnected)
     * - It's missing entirely for a node that should have one
     *
     * @returns true if any subscribers need to be recreated
     */
    hasUnhealthySubscribers() {
        const hasFailedSubscribers = Array.from(this.shardedSubscribers.values()).some((sub) => !sub.isHealthy());
        const hasMissingSubscribers = Array.from(this.subscriberToSlotsIndex.keys()).some((nodeKey) => !this.shardedSubscribers.has(nodeKey));
        return hasFailedSubscribers || hasMissingSubscribers;
    }
    shouldStartSubscriber(sub) {
        if (sub.isStarted()) {
            return false;
        }
        if (!sub.isLazyConnect()) {
            return true;
        }
        const subscriberSlots = this.subscriberToSlotsIndex.get(sub.getNodeKey());
        if (!subscriberSlots) {
            return false;
        }
        return subscriberSlots.some((slot) => {
            const channels = this.channels.get(slot);
            return Boolean(channels && channels.length > 0);
        });
    }
}
exports.default = ClusterSubscriberGroup;
// Retry strategy
ClusterSubscriberGroup.MAX_RETRY_ATTEMPTS = 10;
ClusterSubscriberGroup.MAX_BACKOFF_MS = 2000;
ClusterSubscriberGroup.BASE_BACKOFF_MS = 100;
