macro_rules! dyn_creation_method_declarations {
    () => {
        fn reconcile_creation_metadata<'a>(
            &'a self,
            repair: &'a CreationMetadataRepair,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>>;

        fn publish_creation_coverage<'a>(
            &'a self,
            publication: &'a CreationCoveragePublication,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>>;

        fn commit_first_event<'a>(
            &'a self,
            commit: &'a FirstEventCommit,
        ) -> EventStoreFuture<'a, Result<u64, PersistenceError>>;

        fn create_or_verify<'a>(
            &'a self,
            request: &'a CreateOrVerifyRequest,
        ) -> EventStoreFuture<'a, Result<CreateOrVerifyStoreOutcome, PersistenceError>>;

        fn acknowledge_create_or_verify_notification<'a>(
            &'a self,
            request: &'a CreateOrVerifyRequest,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>>;

        fn list_creation_source_ids_by_type<'a>(
            &'a self,
            tenant: &'a str,
            entity_type: &'a str,
        ) -> EventStoreFuture<'a, Result<Vec<String>, PersistenceError>>;

        fn creation_source_write_version<'a>(
            &'a self,
            tenant: &'a str,
            entity_type: &'a str,
        ) -> EventStoreFuture<'a, Result<u64, PersistenceError>>;
    };
}

macro_rules! dyn_creation_method_implementations {
    () => {
        fn reconcile_creation_metadata<'a>(
            &'a self,
            repair: &'a CreationMetadataRepair,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>> {
            Box::pin(EventStore::reconcile_creation_metadata(self, repair))
        }

        fn publish_creation_coverage<'a>(
            &'a self,
            publication: &'a CreationCoveragePublication,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>> {
            Box::pin(EventStore::publish_creation_coverage(self, publication))
        }

        fn commit_first_event<'a>(
            &'a self,
            commit: &'a FirstEventCommit,
        ) -> EventStoreFuture<'a, Result<u64, PersistenceError>> {
            Box::pin(EventStore::commit_first_event(self, commit))
        }

        fn create_or_verify<'a>(
            &'a self,
            request: &'a CreateOrVerifyRequest,
        ) -> EventStoreFuture<'a, Result<CreateOrVerifyStoreOutcome, PersistenceError>> {
            Box::pin(EventStore::create_or_verify(self, request))
        }

        fn acknowledge_create_or_verify_notification<'a>(
            &'a self,
            request: &'a CreateOrVerifyRequest,
        ) -> EventStoreFuture<'a, Result<(), PersistenceError>> {
            Box::pin(EventStore::acknowledge_create_or_verify_notification(
                self, request,
            ))
        }

        fn list_creation_source_ids_by_type<'a>(
            &'a self,
            tenant: &'a str,
            entity_type: &'a str,
        ) -> EventStoreFuture<'a, Result<Vec<String>, PersistenceError>> {
            Box::pin(EventStore::list_creation_source_ids_by_type(
                self,
                tenant,
                entity_type,
            ))
        }

        fn creation_source_write_version<'a>(
            &'a self,
            tenant: &'a str,
            entity_type: &'a str,
        ) -> EventStoreFuture<'a, Result<u64, PersistenceError>> {
            Box::pin(EventStore::creation_source_write_version(
                self,
                tenant,
                entity_type,
            ))
        }
    };
}

macro_rules! boxed_creation_methods {
    () => {
        /// Box an owned event store behind the object-safe adapter.
        pub fn new<T>(store: T) -> Self
        where
            T: EventStore,
        {
            Self(Arc::new(store))
        }

        /// Box a shared event store behind the object-safe adapter.
        pub fn from_arc<T>(store: Arc<T>) -> Self
        where
            T: EventStore,
        {
            Self(store)
        }

        /// Return the shared object-safe event store.
        pub fn inner(&self) -> Arc<dyn DynEventStore> {
            self.0.clone()
        }

        /// Reconcile one legacy stream's contract and exact current key set.
        pub async fn reconcile_creation_metadata(
            &self,
            repair: &CreationMetadataRepair,
        ) -> Result<(), PersistenceError> {
            self.0.reconcile_creation_metadata(repair).await
        }

        /// Publish a stable full-pass creation coverage proof.
        pub async fn publish_creation_coverage(
            &self,
            publication: &CreationCoveragePublication,
        ) -> Result<(), PersistenceError> {
            self.0.publish_creation_coverage(publication).await
        }

        /// Co-commit an ordinary entity's first event and immutable creation metadata.
        pub async fn commit_first_event(
            &self,
            commit: &FirstEventCommit,
        ) -> Result<u64, PersistenceError> {
            self.0.commit_first_event(commit).await
        }

        /// Atomically create a first event or compare the preserved creation contract.
        pub async fn create_or_verify(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> Result<CreateOrVerifyStoreOutcome, PersistenceError> {
            self.0.create_or_verify(request).await
        }

        /// Durably acknowledge delivery of a pending Created notification.
        pub async fn acknowledge_create_or_verify_notification(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> Result<(), PersistenceError> {
            self.0
                .acknowledge_create_or_verify_notification(request)
                .await
        }

        /// List durable source stream identifiers for creation reconciliation.
        pub async fn list_creation_source_ids_by_type(
            &self,
            tenant: &str,
            entity_type: &str,
        ) -> Result<Vec<String>, PersistenceError> {
            self.0
                .list_creation_source_ids_by_type(tenant, entity_type)
                .await
        }

        /// Return the type-wide source write version used to fence reconciliation.
        pub async fn creation_source_write_version(
            &self,
            tenant: &str,
            entity_type: &str,
        ) -> Result<u64, PersistenceError> {
            self.0
                .creation_source_write_version(tenant, entity_type)
                .await
        }
    };
}
