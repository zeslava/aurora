# Сборочное окружение Android-оболочки: JDK 17 + Android SDK + Gradle.
# Локально SDK не нужен — scripts/build-android.sh собирает APK в этом образе.
FROM eclipse-temurin:17-jdk

ARG CMDLINE_TOOLS=11076708
ARG GRADLE_VERSION=8.11.1

RUN apt-get update \
 && apt-get install -y --no-install-recommends unzip wget imagemagick fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

ENV ANDROID_HOME=/opt/android-sdk
ENV PATH=$PATH:/opt/android-sdk/cmdline-tools/latest/bin:/opt/gradle/bin

RUN wget -q https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS}_latest.zip -O /tmp/tools.zip \
 && mkdir -p $ANDROID_HOME/cmdline-tools \
 && unzip -q /tmp/tools.zip -d $ANDROID_HOME/cmdline-tools \
 && mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest \
 && rm /tmp/tools.zip \
 && yes | sdkmanager --licenses > /dev/null \
 && sdkmanager "platforms;android-35" "build-tools;34.0.0" "platform-tools" > /dev/null

RUN wget -q https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip -O /tmp/gradle.zip \
 && unzip -q /tmp/gradle.zip -d /opt \
 && mv /opt/gradle-${GRADLE_VERSION} /opt/gradle \
 && rm /tmp/gradle.zip
