Here are more comprehensive benchmarks. This is comparison with the next fastest JS projects using the benchmark tool from `msgpack-lite` (and data is from some clinical research data we use that has a good mix of different value types and structures). It also includes comparison to V8 native JSON functionality, and JavaScript Avro (`avsc`, a very optimized Avro implementation):

---------------------------------------------------------- | ------: | ----: | -----: | -----:
msgpackr w/ shared structures: packr.pack(obj);            |  254700 |  5001 |  50929
msgpackr w/ shared structures: packr.unpack(buf);          |  711700 |  5000 | 142340
require("msgpackr").pack(obj);                             |  234000 |  5000 |  46800
require("msgpackr").unpack(buf);                           |  186500 |  5000 |  37300
buf = Buffer(JSON.stringify(obj));                         |  297900 |  5000 |  59580
obj = JSON.parse(buf);                                     |  216600 |  5001 |  43311
buf = require("msgpack-lite").encode(obj);                 |  114000 |  5001 |  22795
obj = require("msgpack-lite").decode(buf);                 |   40700 |  5006 |   8130
buf = require("@msgpack/msgpack").encode(obj);             |  166100 |  5000 |  33220
obj = require("@msgpack/msgpack").decode(buf);             |  136500 |  5002 |  27289
buf = require("msgpack-js-v5").encode(obj);                |   41600 |  5000 |   8320
obj = require("msgpack-js-v5").decode(buf);                |   70200 |  5004 |  14028
buf = require("msgpack-js").encode(obj);                   |   40400 |  5012 |   8060
obj = require("msgpack-js").decode(buf);                   |   67100 |  5002 |  13414
buf = require("msgpack5")().encode(obj);                   |   15800 |  5024 |   3144
obj = require("msgpack5")().decode(buf);                   |   30600 |  5004 |   6115
buf = require("notepack").encode(obj);                     |  125100 |  5002 |  25009
obj = require("notepack").decode(buf);                     |   98600 |  5004 |  19704
require("what-the-pack")... encoder.encode(obj);           |  150300 |  5001 |  30053
require("what-the-pack")... encoder.decode(buf);           |  100100 |  5000 |  20020
obj = require("msgpack-unpack").decode(buf);               |   14900 |  5031 |   2961
require("avsc")...make schema/type...type.toBuffer(obj);   |  266600 |  5000 |  53320
require("avsc")...make schema/type...type.fromBuffer(obj); |  370200 |  5000 |  74040

(`avsc` is schema-based and more comparable in style to msgpackr with shared structures).

(note that benchmarks below are several years old)

Here is a benchmark of streaming data (again borrowed from `msgpack-lite`'s benchmarking), where msgpackr is able to take advantage of the structured record extension and really pull away from other tools:

operation (1000000 x 2)                          |   op    |  ms   |  op/s
------------------------------------------------ | ------: | ----: | -----:
new PackrStream().write(obj);                    | 1000000 |   372 | 2688172
new UnpackrStream().write(buf);                  | 1000000 |   247 | 4048582
stream.write(msgpack.encode(obj));               | 1000000 |  2898 | 345065
stream.write(msgpack.decode(buf));               | 1000000 |  1969 | 507872
stream.write(notepack.encode(obj));              | 1000000 |   901 | 1109877
stream.write(notepack.decode(buf));              | 1000000 |  1012 | 988142
msgpack.Encoder().on("data",ondata).encode(obj); | 1000000 |  1763 | 567214
msgpack.createDecodeStream().write(buf);         | 1000000 |  2222 | 450045
msgpack.createEncodeStream().write(obj);         | 1000000 |  1577 | 634115
msgpack.Decoder().on("data",ondata).decode(buf); | 1000000 |  2246 | 445235



These are the benchmarks from notepack package. The larger test data for these benchmarks is very heavily weighted with large binary/buffer data and objects with extreme numbers of keys (much more than I typically see with real-world data, but YMMV):

node ./benchmarks/encode

library          |   tiny            |  small          | medium         | large
---------------- | ----------------: | --------------: | ---------------| -------:
notepack         | 2,171,621 ops/sec | 546,905 ops/sec | 29,578 ops/sec | 265 ops/sec   
msgpack-js       | 967,682 ops/sec   | 184,455 ops/sec | 20,556 ops/sec | 259 ops/sec   
msgpackr         | 2,392,826 ops/sec | 556,915 ops/sec | 70,573 ops/sec | 313 ops/sec   
msgpack-lite     | 553,143 ops/sec   | 132,318 ops/sec | 11,816 ops/sec | 186 ops/sec   
@msgpack/msgpack | 2,157,655 ops/sec | 573,236 ops/sec | 25,864 ops/sec | 90.26 ops/sec 


node ./benchmarks/decode

library          |   tiny            |  small          | medium          | large
---------------- | ----------------: | --------------: | --------------- | -------:
notepack         | 2,220,904 ops/sec | 560,630 ops/sec | 28,177 ops/sec  | 275 ops/sec   
msgpack-js       | 965,719 ops/sec   | 222,047 ops/sec | 21,431 ops/sec  | 257 ops/sec   
msgpackr         | 2,320,046 ops/sec | 589,167 ops/sec | 70,299 ops/sec  | 329 ops/sec   
msgpackr records | 3,750,547 ops/sec | 912,419 ops/sec | 136,853 ops/sec | 733 ops/sec   
msgpack-lite     | 569,222 ops/sec   | 129,008 ops/sec | 12,424 ops/sec  | 180 ops/sec   
@msgpack/msgpack | 2,089,697 ops/sec | 557,507 ops/sec | 20,256 ops/sec  | 85.03 ops/sec 

This was run by adding the msgpackr to the benchmarks for notepack.

All benchmarks were performed on Node 14.8.0 (Windows i7-4770 3.4Ghz). They can be run with:
npm install --no-save msgpack msgpack-js @msgpack/msgpack msgpack-lite notepack avsc
node tests/benchmark
