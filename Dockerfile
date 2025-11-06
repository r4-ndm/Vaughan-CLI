FROM golang:1.21-alpine AS builder

# Install foundry
RUN apk add --no-cache git ca-certificates curl && \
    curl -L https://foundry.paradigm.xyz | bash && \
    /root/.foundry/bin/foundryup

# Set working directory
WORKDIR /app

# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download

# Copy source code
COPY . .

# Build binary
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o vaughan-crush ./cmd/cli

# Final stage
FROM alpine:latest

# Install ca-certificates for HTTPS requests
RUN apk --no-cache add ca-certificates

# Create non-root user
RUN addgroup -g 1001 -S vaughan && \
    adduser -u 1001 -S vaughan -G vaughan

WORKDIR /home/vaughan

# Copy binary from builder stage
COPY --from=builder /app/vaughan-crush .
COPY --from=builder /root/.foundry/bin/foundryup /usr/local/bin/
COPY --from=builder /root/.foundry/bin/cast /usr/local/bin/
COPY --from=builder /root/.foundry/bin/forge /usr/local/bin/

# Change ownership
RUN chown -R vaughan:vaughan /home/vaughan

# Switch to non-root user
USER vaughan

# Expose port (if needed for future features)
EXPOSE 8080

# Entry point
ENTRYPOINT ["./vaughan-crush"]
CMD ["--help"]