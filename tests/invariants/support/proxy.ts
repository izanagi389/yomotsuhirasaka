// 出口のプロキシの代わり（INV-WH-05）。HTTPSの中継の依頼（CONNECT）と、HTTPの中継の依頼を記録し、そのまま中継する。

import { createServer, request as httpRequest, type Server } from "node:http";
import { connect, type AddressInfo, type Socket } from "node:net";

export interface ProxyRequest {
  method: string;
  /** CONNECT なら `ホスト:ポート`、それ以外は依頼されたURL。 */
  target: string;
}

export interface RecordingProxy {
  readonly port: number;
  readonly requests: ProxyRequest[];
  url(host: string): string;
  close(): Promise<void>;
}

export async function startRecordingProxy(): Promise<RecordingProxy> {
  const requests: ProxyRequest[] = [];
  const sockets = new Set<Socket>();

  const server: Server = createServer((req, res) => {
    requests.push({ method: req.method ?? "", target: req.url ?? "" });
    let target: URL;
    try {
      target = new URL(req.url ?? "");
    } catch {
      res.writeHead(400).end();
      return;
    }
    const upstream = httpRequest(
      { host: target.hostname, port: target.port || 80, path: target.pathname + target.search, method: req.method, headers: req.headers },
      (up) => {
        res.writeHead(up.statusCode ?? 502, up.headers);
        up.pipe(res);
      },
    );
    upstream.on("error", () => res.writeHead(502).end());
    req.pipe(upstream);
  });

  server.on("connect", (req, clientSocket: Socket, head: Buffer) => {
    const target = req.url ?? "";
    requests.push({ method: "CONNECT", target });
    const i = target.lastIndexOf(":");
    const host = target.slice(0, i).replace(/^\[|\]$/g, "");
    const port = Number(target.slice(i + 1));
    const upstream = connect(port, host, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      if (head.length > 0) upstream.write(head);
      upstream.pipe(clientSocket);
      clientSocket.pipe(upstream);
    });
    sockets.add(upstream);
    sockets.add(clientSocket);
    const fail = () => {
      clientSocket.destroy();
      upstream.destroy();
    };
    upstream.on("error", fail);
    clientSocket.on("error", fail);
  });

  await new Promise<void>((resolve) => server.listen(0, "0.0.0.0", resolve));
  const port = (server.address() as AddressInfo).port;
  return {
    port,
    requests,
    url: (host) => `http://${host}:${port}`,
    close: () =>
      new Promise((resolve) => {
        for (const s of sockets) s.destroy();
        server.closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}
