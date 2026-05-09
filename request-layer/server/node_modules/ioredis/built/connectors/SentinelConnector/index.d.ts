/// <reference types="node" />
import { EventEmitter } from "events";
import { NatMap } from "../../cluster/ClusterOptions";
import { ConnectionOptions } from "tls";
import SentinelIterator from "./SentinelIterator";
import { SentinelAddress } from "./types";
import AbstractConnector, { ErrorEmitter } from "../AbstractConnector";
import { NetStream } from "../../types";
interface AddressFromResponse {
    port: string;
    ip: string;
    flags?: string | undefined;
}
declare type PreferredSlaves = ((slaves: AddressFromResponse[]) => AddressFromResponse | null) | Array<{
    port: string;
    ip: string;
    prio?: number | undefined;
}> | {
    port: string;
    ip: string;
    prio?: number | undefined;
};
export { SentinelAddress, SentinelIterator };
export interface SentinelConnectionOptions {
    /**
     * Master group name of the Sentinel
     */
    name?: string | undefined;
    /**
     * @default "master"
     */
    role?: "master" | "slave" | undefined;
    tls?: ConnectionOptions | undefined;
    sentinelUsername?: string | undefined;
    sentinelPassword?: string | undefined;
    sentinels?: Array<Partial<SentinelAddress>> | undefined;
    sentinelRetryStrategy?: ((retryAttempts: number) => number | void | null) | undefined;
    sentinelReconnectStrategy?: ((retryAttempts: number) => number | void | null) | undefined;
    preferredSlaves?: PreferredSlaves | undefined;
    connectTimeout?: number | undefined;
    disconnectTimeout?: number | undefined;
    sentinelCommandTimeout?: number | undefined;
    enableTLSForSentinelMode?: boolean | undefined;
    sentinelTLS?: ConnectionOptions | undefined;
    natMap?: NatMap | undefined;
    updateSentinels?: boolean | undefined;
    /**
     * @default 10
     */
    sentinelMaxConnections?: number | undefined;
    failoverDetector?: boolean | undefined;
}
export default class SentinelConnector extends AbstractConnector {
    protected options: SentinelConnectionOptions;
    emitter: EventEmitter | null;
    protected sentinelIterator: SentinelIterator;
    private retryAttempts;
    private failoverDetector;
    constructor(options: SentinelConnectionOptions);
    check(info: {
        role?: string;
    }): boolean;
    disconnect(): void;
    connect(eventEmitter: ErrorEmitter): Promise<NetStream>;
    private updateSentinels;
    private resolveMaster;
    private resolveSlave;
    private sentinelNatResolve;
    private connectToSentinel;
    private resolve;
    private initFailoverDetector;
}
