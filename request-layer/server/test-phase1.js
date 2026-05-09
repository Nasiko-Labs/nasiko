const http = require("http");

const data = JSON.stringify({ agent: "mock-translator", query: "Hello world" });

const options = {
  hostname: "localhost",
  port: 3000,
  path: "/api/process",
  method: "POST",
  headers: { "Content-Type": "application/json", "Content-Length": data.length },
};

const req = http.request(options, (res) => {
  let body = "";
  res.on("data", (chunk) => (body += chunk));
  res.on("end", () => {
    console.log("=== FIRST REQUEST (cache miss expected) ===");
    console.log(JSON.stringify(JSON.parse(body), null, 2));

    // Send the same request again (cache hit expected)
    const req2 = http.request(options, (res2) => {
      let body2 = "";
      res2.on("data", (chunk) => (body2 += chunk));
      res2.on("end", () => {
        console.log("\n=== SECOND REQUEST (cache hit expected) ===");
        console.log(JSON.stringify(JSON.parse(body2), null, 2));

        // Check cache stats
        http.get("http://localhost:3000/api/cache/stats", (res3) => {
          let body3 = "";
          res3.on("data", (chunk) => (body3 += chunk));
          res3.on("end", () => {
            console.log("\n=== CACHE STATS ===");
            console.log(JSON.stringify(JSON.parse(body3), null, 2));
          });
        });
      });
    });
    req2.write(data);
    req2.end();
  });
});

req.write(data);
req.end();
