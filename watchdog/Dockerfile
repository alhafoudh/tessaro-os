# The image that runs on the device: Ruby 4 with the watchdog and its gems.
# Built by `mise run watchdog-image`, which also exports it for the image
# recipe (meta-tessaro-distro/recipes-browser/tessaro-kiosk/files/).
FROM ruby:4.0-slim-bookworm

WORKDIR /app

COPY Gemfile Gemfile.lock ./
RUN bundle config set --local without test && bundle install

COPY lib ./lib
COPY bin ./bin

ENTRYPOINT ["ruby", "bin/tessaro-kiosk-watchdog"]
