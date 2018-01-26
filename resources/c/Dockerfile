FROM frolvlad/alpine-gcc
COPY code.c /
ENV TERM xterm
RUN gcc code.c -o code
CMD ["/code"]
