import http.server
import socketserver
import urllib.request
import json

PORT = 4005
ROUTER_URL = "http://localhost:8081"
KONG_URL = "http://localhost:9100"

class ProxyHandler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS, PUT, DELETE')
        self.send_header('Access-Control-Allow-Headers', '*')
        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(200, "ok")
        self.end_headers()

    def handle_login(self):
        payloads = [
            json.dumps({"email": "superuser-email", "password": "WBmc60fKx_M7GpEh14IsWP2Us6WnMFLm-IZHxVtfcqE"}).encode("utf-8"),
            json.dumps({"access_key": "NASK_zzonHU_UdNhK1ZG-0GFGzA", "access_secret": "WBmc60fKx_M7GpEh14IsWP2Us6WnMFLm-IZHxVtfcqE"}).encode("utf-8"),
            json.dumps({"email": "admin@nasiko.com", "password": "password"}).encode("utf-8"),
            json.dumps({"email": "admin@nasiko.com", "password": "admin123"}).encode("utf-8")
        ]
        urls_to_try = [
            "http://localhost:8000/api/v1/auth/login",
            "http://localhost:8082/api/v1/auth/login",
            "http://localhost:8082/auth/login",
            "http://localhost:9100/auth/login",
            "http://localhost:9100/api/v1/auth/login"
        ]
        for data in payloads:
            for url in urls_to_try:
                req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
                try:
                    with urllib.request.urlopen(req, timeout=3) as res:
                        if res.getcode() == 200:
                            body = res.read().decode()
                            self.send_response(200)
                            self.send_header("Content-Type", "application/json")
                            self.end_headers()
                            self.wfile.write(body.encode())
                            print(f"✅ Auto-login succeeded via {url}")
                            return
                except Exception:
                    pass
        
        self.send_response(500)
        self.end_headers()
        self.wfile.write(b'{"error": "Failed to auto-login to any backend URL."}')

    def handle_proxy(self, target_base):
        url = target_base + self.path
        req = urllib.request.Request(url, method=self.command)
        
        for key, val in self.headers.items():
            if key.lower() not in ['host', 'origin', 'referer', 'connection']:
                req.add_header(key, val)

        if self.command in ['POST', 'PUT']:
            length = int(self.headers.get('Content-Length', 0))
            if length > 0:
                req.data = self.rfile.read(length)

        try:
            with urllib.request.urlopen(req) as response:
                self.send_response(response.getcode())
                for key, val in response.getheaders():
                    if key.lower() not in ['transfer-encoding', 'connection']:
                        self.send_header(key, val)
                self.end_headers()
                self.wfile.write(response.read())
        except urllib.error.HTTPError as e:
            self.send_response(e.code)
            for key, val in e.headers.items():
                if key.lower() not in ['transfer-encoding', 'connection']:
                    self.send_header(key, val)
            self.end_headers()
            self.wfile.write(e.read())
        except Exception as e:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(str(e).encode())

    def do_GET(self):
        if self.path.startswith('/api/router'):
            self.path = self.path.replace('/api/router', '')
            self.handle_proxy(ROUTER_URL)
        elif self.path.startswith('/api/kong'):
            self.path = self.path.replace('/api/kong', '')
            self.handle_proxy(KONG_URL)
        else:
            if self.path == '/':
                self.path = '/dashboard.html'
            super().do_GET()

    def do_POST(self):
        if self.path == '/api/auto-login':
            self.handle_login()
        elif self.path.startswith('/api/router'):
            self.path = self.path.replace('/api/router', '')
            self.handle_proxy(ROUTER_URL)
        elif self.path.startswith('/api/kong'):
            self.path = self.path.replace('/api/kong', '')
            self.handle_proxy(KONG_URL)

    def do_PUT(self):
        if self.path.startswith('/api/router'):
            self.path = self.path.replace('/api/router', '')
            self.handle_proxy(ROUTER_URL)

with socketserver.TCPServer(("", PORT), ProxyHandler) as httpd:
    print(f"✅ Dashboard Server running at http://localhost:{PORT}")
    print("Press Ctrl+C to stop.")
    httpd.serve_forever()
