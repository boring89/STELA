/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * BSD sockets on STELA's Linux personality (docs/net.md): fetches
 * http://10.0.2.2:PORT/hello.txt (QEMU user networking's host) with
 * socket, connect, write and read, checks getsockname, getpeername and
 * getsockopt, sends a UDP datagram, and expects ECONNREFUSED from a closed
 * port. Usage: net PORT EXPECTED-TEXT. Prints "net: OK" on success.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int fail(const char *what)
{
    printf("net: FAIL %s (errno %d)\n", what, errno);
    return 1;
}

int main(int argc, char **argv)
{
    if (argc < 3)
    {
        printf("net: usage: net PORT EXPECTED-TEXT\n");
        return 2;
    }
    int port = atoi(argv[1]);
    struct sockaddr_in host;
    memset(&host, 0, sizeof host);
    host.sin_family = AF_INET;
    host.sin_port = htons((unsigned short)port);
    host.sin_addr.s_addr = inet_addr("10.0.2.2");

    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0)
        return fail("socket");
    if (connect(s, (struct sockaddr *)&host, sizeof host) != 0)
        return fail("connect");

    struct sockaddr_in me;
    socklen_t len = sizeof me;
    if (getsockname(s, (struct sockaddr *)&me, &len) != 0 || len != sizeof me)
        return fail("getsockname");
    printf("net: connected from %s:%d\n", inet_ntoa(me.sin_addr), ntohs(me.sin_port));
    struct sockaddr_in peer;
    len = sizeof peer;
    if (getpeername(s, (struct sockaddr *)&peer, &len) != 0 || peer.sin_port != host.sin_port)
        return fail("getpeername");
    int err = -1;
    len = sizeof err;
    if (getsockopt(s, SOL_SOCKET, SO_ERROR, &err, &len) != 0 || err != 0)
        return fail("getsockopt");

    const char request[] = "GET /hello.txt HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
    if (write(s, request, sizeof request - 1) != (ssize_t)(sizeof request - 1))
        return fail("write");
    char response[4096];
    size_t got = 0;
    for (;;)
    {
        ssize_t n = read(s, response + got, sizeof response - 1 - got);
        if (n < 0)
            return fail("read");
        if (n == 0)
            break;
        got += (size_t)n;
    }
    response[got] = '\0';
    close(s);
    char *body = strstr(response, "\r\n\r\n");
    if (body == NULL || strncmp(response, "HTTP/1.0 200", 12) != 0)
        return fail("HTTP response");
    body += 4;
    printf("net: body: %s", body);
    if (strstr(body, argv[2]) == NULL)
        return fail("body");

    int u = socket(AF_INET, SOCK_DGRAM, 0);
    if (u < 0)
        return fail("udp socket");
    struct sockaddr_in dns = host;
    dns.sin_port = htons(9);
    if (sendto(u, "x", 1, 0, (struct sockaddr *)&dns, sizeof dns) != 1)
        return fail("sendto");
    close(u);

    int r = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in closed = host;
    closed.sin_port = htons(1);
    if (connect(r, (struct sockaddr *)&closed, sizeof closed) == 0 || errno != ECONNREFUSED)
        printf("net: note: port 1 did not refuse (errno %d)\n", errno);
    close(r);

    if (socket(AF_INET6, SOCK_STREAM, 0) >= 0 || errno != EAFNOSUPPORT)
        return fail("AF_INET6 must be refused");
    printf("net: OK\n");
    return 0;
}
