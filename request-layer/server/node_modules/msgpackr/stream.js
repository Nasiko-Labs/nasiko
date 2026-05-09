import { Transform } from 'stream';
import { Packr } from './pack.js';
import { Unpackr } from './unpack.js';
var DEFAULT_OPTIONS = {objectMode: true};

export class PackrStream extends Transform {
	constructor(options) {
		if (!options)
			options = {};
		options.writableObjectMode = true;
		super(options);
		options.sequential = true;
		this.packr = options.packr || new Packr(options);
	}
	_transform(value, encoding, callback) {
		this.push(this.packr.pack(value));
		callback();
	}
}

export class UnpackrStream extends Transform {
	constructor(options) {
		if (!options)
			options = {};
		options.objectMode = true;
		super(options);
		options.structures = [];
		this.maxIncompleteBufferSize = options.maxIncompleteBufferSize !== undefined ? options.maxIncompleteBufferSize : 0x4000000;
		this.unpackr = options.unpackr || new Unpackr(options);
	}
	_transform(chunk, encoding, callback) {
		if (this.incompleteBuffer) {
			chunk = Buffer.concat([this.incompleteBuffer, chunk]);
			this.incompleteBuffer = null;
		}
		let values;
		try {
			values = this.unpackr.unpackMultiple(chunk);
		} catch(error) {
			if (error.incomplete) {
				let incompleteBuffer = chunk.slice(error.lastPosition);
				if (incompleteBuffer.length > this.maxIncompleteBufferSize) {
					this.incompleteBuffer = null;
					return callback(new Error('Maximum incomplete buffer size exceeded'));
				}
				this.incompleteBuffer = incompleteBuffer;
				values = error.values;
			} else {
				return callback(error);
			}
		}
		for (let value of values || []) {
			if (value === null)
				value = this.getNullValue();
			this.push(value);
		}
		callback();
	}
	getNullValue() {
		return Symbol.for(null);
	}
}
