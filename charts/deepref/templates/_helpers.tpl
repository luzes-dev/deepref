{{- define "deepref.name" -}}
deepref
{{- end -}}

{{- define "deepref.fullname" -}}
{{- if eq .Release.Name "deepref" -}}deepref{{- else -}}{{ printf "%s-deepref" .Release.Name | trunc 63 | trimSuffix "-" }}{{- end -}}
{{- end -}}

{{- define "deepref.namespace" -}}
{{- default .Release.Namespace .Values.namespace.name -}}
{{- end -}}

{{- define "deepref.componentName" -}}
{{- printf "%s-%s" (include "deepref.fullname" .root) .component | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "deepref.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
app.kubernetes.io/name: {{ include "deepref.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: deepref
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
deepref.io/environment: {{ .Values.global.environment }}
{{- end -}}

{{- define "deepref.selectorLabels" -}}
app.kubernetes.io/name: {{ include "deepref.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "deepref.image" -}}
{{- $repository := required (printf "images.%s.repository is required" .name) .image.repository -}}
{{- $digest := required (printf "images.%s.digest is required" .name) .image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "images.%s.digest must be a sha256 digest" .name) -}}
{{- end -}}
{{- printf "%s@%s" $repository $digest -}}
{{- end -}}

{{- define "deepref.podSecurityContext" -}}
runAsNonRoot: true
seccompProfile:
  type: RuntimeDefault
{{- end -}}

{{- define "deepref.containerSecurityContext" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
capabilities:
  drop:
    - ALL
{{- end -}}

{{- define "deepref.imagePullSecrets" -}}
{{- with .Values.global.imagePullSecrets }}
imagePullSecrets:
{{- range . }}
  - name: {{ . }}
{{- end }}
{{- end }}
{{- end -}}

{{- define "deepref.topology" -}}
topologySpreadConstraints:
  - maxSkew: 1
    topologyKey: topology.kubernetes.io/zone
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
{{ include "deepref.selectorLabels" . | indent 8 }}
  - maxSkew: 1
    topologyKey: kubernetes.io/hostname
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
{{ include "deepref.selectorLabels" . | indent 8 }}
{{- end -}}

{{- define "deepref.runtimeEnv" -}}
- name: APP_ENV
  value: {{ .root.Values.global.environment | quote }}
- name: DATABASE_URL_FILE
  value: /var/run/secrets/deepref/database/url
- name: DATABASE_POOL_MIN
  value: {{ .root.Values.runtime.database.poolMin | quote }}
- name: DATABASE_POOL_MAX
  value: {{ .root.Values.runtime.database.poolMax | quote }}
- name: DATABASE_ACQUIRE_TIMEOUT_SECS
  value: {{ .root.Values.runtime.database.acquireTimeoutSeconds | quote }}
- name: DATABASE_IDLE_TIMEOUT_SECS
  value: {{ .root.Values.runtime.database.idleTimeoutSeconds | quote }}
- name: DATABASE_MAX_LIFETIME_SECS
  value: {{ .root.Values.runtime.database.maxLifetimeSeconds | quote }}
- name: NATS_URL
  value: tls://{{ include "deepref.componentName" (dict "root" .root "component" "nats") }}:4222
- name: NATS_CREDENTIALS_FILE
  value: /var/run/secrets/deepref/nats/credentials
- name: NATS_CA_FILE
  value: /var/run/secrets/deepref/nats-ca/ca.crt
- name: NATS_CONNECT_TIMEOUT_SECS
  value: {{ .root.Values.runtime.nats.connectTimeoutSeconds | quote }}
- name: NATS_REQUEST_TIMEOUT_SECS
  value: {{ .root.Values.runtime.nats.requestTimeoutSeconds | quote }}
- name: NATS_WORKER_CONSUMER
  value: deepref-worker
- name: NATS_PROJECTOR_CONSUMER
  value: deepref-projector
- name: NEO4J_URI
  value: bolt://{{ include "deepref.componentName" (dict "root" .root "component" "neo4j") }}:7687
- name: NEO4J_USER
  valueFrom:
    secretKeyRef:
      name: {{ include "deepref.componentName" (dict "root" .root "component" "neo4j-auth") }}
      key: username
- name: NEO4J_PASSWORD_FILE
  value: /var/run/secrets/deepref/neo4j/password
- name: NEO4J_POOL_MAX
  value: {{ .root.Values.runtime.neo4j.poolMax | quote }}
- name: NEO4J_CONNECT_TIMEOUT_SECS
  value: {{ .root.Values.runtime.neo4j.connectTimeoutSeconds | quote }}
- name: NEO4J_QUERY_TIMEOUT_SECS
  value: {{ .root.Values.runtime.neo4j.queryTimeoutSeconds | quote }}
- name: OTEL_SERVICE_NAME
  value: {{ .serviceName | quote }}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: http://{{ include "deepref.componentName" (dict "root" .root "component" "adot") }}:4317
- name: SHUTDOWN_DEADLINE_SECS
  value: {{ .root.Values.runtime.shutdownDeadlineSeconds | quote }}
{{- end -}}

{{- define "deepref.runtimeVolumeMounts" -}}
- name: tmp
  mountPath: /tmp
- name: database-secret
  mountPath: /var/run/secrets/deepref/database
  readOnly: true
- name: nats-credentials
  mountPath: /var/run/secrets/deepref/nats
  readOnly: true
- name: nats-ca
  mountPath: /var/run/secrets/deepref/nats-ca
  readOnly: true
- name: neo4j-secret
  mountPath: /var/run/secrets/deepref/neo4j
  readOnly: true
{{- end -}}

{{- define "deepref.runtimeVolumes" -}}
- name: tmp
  emptyDir:
    sizeLimit: 128Mi
- name: database-secret
  secret:
    secretName: {{ include "deepref.componentName" (dict "root" .root "component" "database") }}
    items:
      - key: url
        path: url
- name: nats-credentials
  secret:
    secretName: {{ include "deepref.componentName" (dict "root" .root "component" (printf "nats-%s" .credential)) }}
    items:
      - key: credentials
        path: credentials
- name: nats-ca
  secret:
    secretName: {{ include "deepref.componentName" (dict "root" .root "component" "nats-tls") }}
    items:
      - key: ca.crt
        path: ca.crt
- name: neo4j-secret
  secret:
    secretName: {{ include "deepref.componentName" (dict "root" .root "component" "neo4j-auth") }}
    items:
      - key: password
        path: password
{{- end -}}
