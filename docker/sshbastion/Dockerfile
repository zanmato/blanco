# Minimal SSH bastion for integration tests. Unlike hardened prebuilt images
# (e.g. linuxserver/openssh-server, which ships `AllowTcpForwarding no`), this
# explicitly enables TCP forwarding so the test tunnel can open `direct-tcpip`
# channels to the sibling database services on the compose network.
FROM alpine:3.20

RUN apk add --no-cache openssh-server \
    && ssh-keygen -A \
    && mkdir -p /run/sshd \
    && adduser -D -s /bin/sh blanco \
    && echo 'blanco:blanco' | chpasswd \
    && sed -i \
        -e 's/^#*AllowTcpForwarding.*/AllowTcpForwarding yes/' \
        -e 's/^#*PasswordAuthentication.*/PasswordAuthentication yes/' \
        /etc/ssh/sshd_config

EXPOSE 22

# `-e` logs to stderr so `docker compose logs sshbastion` shows auth/forwarding
# failures during test debugging.
CMD ["/usr/sbin/sshd", "-D", "-e"]
