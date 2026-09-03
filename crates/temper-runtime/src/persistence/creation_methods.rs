macro_rules! impl_creation_event_store_methods {
    () => {
        /// Reconcile one pre-feature stream from its immutable first event and an
        /// exact authoritative key projection.
        fn reconcile_creation_metadata(
            &self,
            repair: &CreationMetadataRepair,
        ) -> impl std::future::Future<Output = Result<(), PersistenceError>> + Send {
            let _ = repair;
            async {
                Err(PersistenceError::Storage(
                    "creation metadata reconciliation is unsupported by this event store".into(),
                ))
            }
        }

        /// Publish a stable full-pass creation coverage proof.
        fn publish_creation_coverage(
            &self,
            publication: &CreationCoveragePublication,
        ) -> impl std::future::Future<Output = Result<(), PersistenceError>> + Send {
            let _ = publication;
            async {
                Err(PersistenceError::Storage(
                    "creation coverage publication is unsupported by this event store".into(),
                ))
            }
        }

        /// Atomically commit an ordinary entity's first event and creation metadata.
        fn commit_first_event(
            &self,
            commit: &FirstEventCommit,
        ) -> impl std::future::Future<Output = Result<u64, PersistenceError>> + Send {
            async move {
                commit.validate()?;
                Err(PersistenceError::Storage(
                    "atomic first-event metadata commit is unsupported by this event store".into(),
                ))
            }
        }

        /// Atomically create sequence one or compare with the resolved owner.
        fn create_or_verify(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> impl std::future::Future<Output = Result<CreateOrVerifyStoreOutcome, PersistenceError>> + Send {
            let _ = request;
            async {
                Err(PersistenceError::Storage(
                    "create-or-verify is unsupported by this event store".into(),
                ))
            }
        }

        /// Acknowledge publication after a durable downstream consumer accepts it.
        fn acknowledge_create_or_verify_notification(
            &self,
            request: &CreateOrVerifyRequest,
        ) -> impl std::future::Future<Output = Result<(), PersistenceError>> + Send {
            let _ = request;
            async {
                Err(PersistenceError::Storage(
                    "create-or-verify notification acknowledgement is unsupported".into(),
                ))
            }
        }
    };
}
