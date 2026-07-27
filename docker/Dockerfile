FROM scratch
ARG TARGETARCH=amd64
COPY dist/${TARGETARCH}/phoenix /phoenix
ENV HOME=/data
WORKDIR /data
VOLUME ["/data"]
ENTRYPOINT ["/phoenix"]
CMD ["serve"]
