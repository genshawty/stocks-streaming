1. get ack from master when sending
2. magic bytes for udp
3. complete send command
4. add ping/pong
4.1. add max retries to send ping to the server
5. handle if some bytes are missing while ping sent
6. handle partial data recieving (maybe need, maybe not)