"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const util_1 = require("./util");
const utils_1 = require("../utils");
const Redis_1 = require("../Redis");
const debug = (0, utils_1.Debug)("cluster:subscriberGroup:shardedSubscriber");
const SubscriberStatus = {
    IDLE: "idle",
    STARTING: "starting",
    CONNECTED: "connected",
    STOPPING: "stopping",
    ENDED: "ended",
};
const ALLOWED_STATUS_UPDATES = {
    [SubscriberStatus.IDLE]: [
        SubscriberStatus.STARTING,
        SubscriberStatus.STOPPING,
        SubscriberStatus.ENDED,
    ],
    [SubscriberStatus.STARTING]: [
        SubscriberStatus.CONNECTED,
        SubscriberStatus.STOPPING,
        SubscriberStatus.ENDED,
    ],
    [SubscriberStatus.CONNECTED]: [
        SubscriberStatus.STOPPING,
        SubscriberStatus.ENDED,
    ],
    [SubscriberStatus.STOPPING]: [SubscriberStatus.ENDED],
    [SubscriberStatus.ENDED]: [],
};
class ShardedSubscriber {
    constructor(emitter, options, redisOptions) {
        var _a;
        this.emitter = emitter;
        this.status = SubscriberStatus.IDLE;
        this.instance = null;
        this.connectPromise = null;
        // Store listener references for cleanup
        this.messageListeners = new Map();
        this.onEnd = () => {
            this.updateStatus(SubscriberStatus.ENDED);
            this.emitter.emit("-node", this.instance, this.nodeKey);
        };
        this.onError = (error) => {
            this.emitter.emit("nodeError", error, this.nodeKey);
        };
        this.onMoved = () => {
            this.emitter.emit("moved");
        };
        this.instance = new Redis_1.default((0, utils_1.defaults)({
            enableReadyCheck: false,
            enableOfflineQueue: true,
            connectionName: (0, util_1.getConnectionName)("ssubscriber", options.connectionName),
            /**
             * Disable auto reconnection for subscribers.
             * The ClusterSubscriberGroup will handle the reconnection.
             */
            retryStrategy: null,
            lazyConnect: true,
        }, options, redisOptions));
        this.lazyConnect = (_a = redisOptions === null || redisOptions === void 0 ? void 0 : redisOptions.lazyConnect) !== null && _a !== void 0 ? _a : true;
        this.nodeKey = (0, util_1.getNodeKey)(options);
        // Register listeners
        this.instance.on("end", this.onEnd);
        this.instance.on("error", this.onError);
        this.instance.on("moved", this.onMoved);
        for (const event of ["smessage", "smessageBuffer"]) {
            const listener = (...args) => {
                this.emitter.emit(event, ...args);
            };
            this.messageListeners.set(event, listener);
            this.instance.on(event, listener);
        }
    }
    async start() {
        if (this.connectPromise) {
            return this.connectPromise;
        }
        if (this.status === SubscriberStatus.STARTING ||
            this.status === SubscriberStatus.CONNECTED) {
            return;
        }
        if (this.status === SubscriberStatus.ENDED || !this.instance) {
            throw new Error(`Sharded subscriber ${this.nodeKey} cannot be restarted once ended.`);
        }
        this.updateStatus(SubscriberStatus.STARTING);
        this.connectPromise = this.instance.connect();
        try {
            await this.connectPromise;
            this.updateStatus(SubscriberStatus.CONNECTED);
        }
        catch (err) {
            this.updateStatus(SubscriberStatus.ENDED);
            throw err;
        }
        finally {
            this.connectPromise = null;
        }
    }
    stop() {
        this.updateStatus(SubscriberStatus.STOPPING);
        if (this.instance) {
            this.instance.disconnect();
            this.instance.removeAllListeners();
            this.messageListeners.clear();
            this.instance = null;
        }
        this.updateStatus(SubscriberStatus.ENDED);
        debug("stopped %s", this.nodeKey);
    }
    isStarted() {
        return [
            SubscriberStatus.CONNECTED,
            SubscriberStatus.STARTING,
        ].includes(this.status);
    }
    get subscriberStatus() {
        return this.status;
    }
    isHealthy() {
        return ((this.status === SubscriberStatus.IDLE ||
            this.status === SubscriberStatus.CONNECTED ||
            this.status === SubscriberStatus.STARTING) &&
            this.instance !== null);
    }
    getInstance() {
        return this.instance;
    }
    getNodeKey() {
        return this.nodeKey;
    }
    isLazyConnect() {
        return this.lazyConnect;
    }
    updateStatus(nextStatus) {
        if (this.status === nextStatus) {
            return;
        }
        if (!ALLOWED_STATUS_UPDATES[this.status].includes(nextStatus)) {
            debug("Invalid status transition for %s: %s -> %s", this.nodeKey, this.status, nextStatus);
            return;
        }
        this.status = nextStatus;
    }
}
exports.default = ShardedSubscriber;
